//
// Copyright (c) 2026, SkyFoundry LLC
// Licensed under the Academic Free License version 3.0
//
// History:
//   23 Feb 2026  Hathi  Creation
//

using concurrent
using haystack
using folio

**
** RustFolioBackup implements FolioBackup for the Rust-backed Folio.
**
** Backup flow:
**  1. Send BACKUP_CREATE RPC → Rust writes a consistent redb snapshot
**     to a caller-specified temp path (logical table copy under a read txn,
**     which pins the MVCC state for the duration — no concurrent write
**     can corrupt the snapshot).
**  2. Fantom zips the snapshot into {name}-YYMMDD-hhmmss.zip in the
**     backup directory ({folio.dir}/../backup/).
**  3. Temp snapshot file is deleted.
**
** Zip format: {name}-YYMMDD-hhmmss.zip
**   Contents: {name}-YYMMDD-hhmmss/db/db.redb
**   This mirrors hxFolio's {basename}/db/ path-prefix convention,
**   making the backup format compatible with tooling that expects it.
**
** monitor() returns null because the RPC call blocks until Rust has
** finished writing the snapshot — there is no intermediate progress to
** report from the Fantom side.
**
@NoDoc
const class RustFolioBackup : FolioBackup
{
  new make(RustFolio folio)
  {
    this.folio = folio
    this.dir   = folio.dir + `../backup/`
  }

  const RustFolio folio
  const File dir

  private const ActorPool pool          := ActorPool.make
  private const AtomicBool inProgressRef := AtomicBool(false)
  private const AtomicRef  lastStatusRef := AtomicRef(null)

//////////////////////////////////////////////////////////////////////////
// FolioBackup
//////////////////////////////////////////////////////////////////////////

  ** List backups ordered from newest to oldest.
  override FolioBackupFile[] list()
  {
    acc := FolioBackupFile[,]
    if (!dir.exists) return acc
    dir.list.each |f|
    {
      if (f.isDir || f.ext != "zip") return
      try
      {
        // Filename format: {proj-name}-YYMMDD-hhmmss.zip
        // Indices from the end of basename (must be ≥15 chars):
        //   [-13..-12] = YY  [-11..-10] = MM  [-9..-8] = DD
        //   [-6..-5]   = hh  [-4..-3]   = mm  [-2..-1] = ss
        s := f.basename
        if (s.size < 15) return
        date := Date(2000+s[-13..-12].toInt, Month.vals[s[-11..-10].toInt-1], s[-9..-8].toInt)
        time := Time(s[-6..-5].toInt, s[-4..-3].toInt, s[-2..-1].toInt)
        ts   := date.toDateTime(time)
        acc.add(FolioBackupFile(f, ts))
      }
      catch {}
    }
    return acc.sortr |a, b| { a.ts <=> b.ts }
  }

  **
  ** Kick off a backup in the background.  Returns a FolioFuture that resolves
  ** when the backup zip has been written.  Throws IOErr if already running.
  **
  ** Implementation:
  **  1. Rust writes a point-in-time redb snapshot to a temp path (via
  **     BACKUP_CREATE RPC, which blocks until the snapshot is complete).
  **  2. Fantom zips the snapshot into the backup directory.
  **  3. Temp file is deleted.
  **
  override FolioFuture create()
  {
    if (!inProgressRef.compareAndSet(false, true))
      throw IOErr("A backup is already in progress")

    ts      := DateTime.now(null).toLocale("YYMMDD-hhmmss")
    name    := folio.name
    zipFile := dir + "${name}-${ts}.zip".toUri
    tmpFile := dir + ".tmp-${ts}.redb".toUri

    if (zipFile.exists)
    {
      inProgressRef.val = false
      throw IOErr("Backup file already exists: $zipFile")
    }

    // Ensure backup directory exists before handing off to the background actor.
    dir.create

    // Completable future resolved by the background actor.
    f := Future.makeCompletable

    // Capture values needed by the closure.  All are const/immutable.
    bFolio  := folio
    bTmp    := tmpFile
    bZip    := zipFile
    bName   := name
    bTs     := ts
    bInProg := inProgressRef
    bStatus := lastStatusRef

    Actor.make(pool) |msg->Obj?|
    {
      try
      {
        // Step 1 — ask Rust for a consistent snapshot at the temp path.
        c := bFolio.connForBackup ?: throw ShutdownErr("RustFolio is closed")
        c.backupCreate(bTmp.osPath)

        // Step 2 — zip the snapshot into the backup directory.
        zip := Zip.write(bZip.out)
        out := zip.writeNext(Uri.fromStr("${bName}-${bTs}/db/db.redb"))
        bTmp.in.pipe(out)  // pipe() closes both bTmp.in and out (finalizes the entry)
        zip.close

        // Step 3 — remove temp snapshot.
        bTmp.delete

        bStatus.val = "last:${bTs}"
        f.complete(CountFolioRes(0))
      }
      catch (Err e)
      {
        try { bTmp.delete } catch {}
        bStatus.val = "error:${e.msg}"
        f.completeErr(e)
      }
      finally
      {
        bInProg.val = false
      }
      return null
    }.send(null)

    return FolioFuture.makeAsync(f)
  }

  **
  ** No in-progress monitor for rustFolio: the BACKUP_CREATE RPC blocks
  ** until Rust has finished writing the snapshot, so there is no partial
  ** progress to expose from the Fantom side.
  **
  override Obj? monitor() { null }

  ** Human-readable status string.
  override Str status()
  {
    if (inProgressRef.val) return "Backup in progress"

    // Check last recorded result from the most recent create() run.
    s := lastStatusRef.val as Str
    if (s != null)
    {
      if (s.startsWith("error:")) return "Backup error: ${s[6..-1]}"
      if (s.startsWith("last:"))
      {
        // Parse stored timestamp for a friendly "today at HH:mm" or date display.
        tsStr := s[5..-1]
        try
        {
          date := Date(2000+tsStr[-13..-12].toInt, Month.vals[tsStr[-11..-10].toInt-1], tsStr[-9..-8].toInt)
          time := Time(tsStr[-6..-5].toInt, tsStr[-4..-3].toInt, tsStr[-2..-1].toInt)
          ts   := date.toDateTime(time)
          when := ts.date == Date.today ? "today at ${ts.time.toLocale}" : ts.date.toLocale
          return "Last backup was $when"
        }
        catch {}
      }
    }

    // Fallback — scan the backup directory.
    files := list
    if (files.isEmpty) return "No backups"
    last := files.first
    when := last.ts.date == Date.today ? "today at ${last.ts.time.toLocale}" : last.ts.date.toLocale
    return "Last backup was $when"
  }

  ** Human-readable summary for one backup file.
  override Str summary(FolioBackupFile file)
  {
    if (inProgressRef.val) return "Backup in progress"
    return Etc.tsToDis(file.ts)
  }

}
