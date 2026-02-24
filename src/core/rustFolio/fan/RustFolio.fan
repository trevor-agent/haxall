//
// Copyright (c) 2026, SkyFoundry LLC
// Licensed under the Academic Free License version 3.0
//
// History:
//   23 Feb 2026  Hathi  Creation
//

using concurrent
using xeto
using haystack
using folio

**
** RustFolio: Rust-backed Folio implementation.
**
** The rust-folio binary handles record persistence (redb), filter
** evaluation, history storage, and display string computation.
** It communicates with the Fantom/Haxall ecosystem via a binary
** protocol over a loopback TCP socket (ephemeral port, 127.0.0.1 only).
**
** Scope boundary (what stays Fantom-side):
**   - PasswordStore (passwords.props file)
**   - FolioWatch and watch lifecycle
**   - Pre/post-commit hook dispatch (hook identity required by tests)
**   - Backup and file storage (not yet implemented)
**
const class RustFolio : Folio
{

//////////////////////////////////////////////////////////////////////////
// Construction
//////////////////////////////////////////////////////////////////////////

  **
  ** Open database for given configuration. Spawns the rust-folio process
  ** and connects to it.
  **
  static Folio open(FolioConfig config)
  {
    folio := make(config)
    return folio
  }

  private new make(FolioConfig config) : super(config)
  {
    passwords = PasswordStore.open(dir + `passwords.props`, config)

    // Spawn the Rust subprocess
    proc := RustFolioProcess(dir)
    port := proc.start(config)

    // Connect over TCP
    conn := RustFolioConn()
    conn.connect(port)

    // Wrap mutable objects in Unsafe so they can be stored in AtomicRef.
    // Unsafe is the Fantom-idiomatic way to hold mutable state in a const class.
    connRef    = AtomicRef(Unsafe(conn))
    processRef = AtomicRef(Unsafe(proc))

    // History implementation (see RustFolioHis)
    hisImpl = RustFolioHis(this)

    // Backup implementation (see RustFolioBackup)
    backupImpl = RustFolioBackup(this)

    // Display string manager — compute initial dis cache on open so that
    // post-reopen verifyDictDis checks work without an explicit syncDis call.
    disMgr = RustFolioDisMgr()
    disMgr.updateAll(conn.readAll(Filter.has("id"), null))
  }

//////////////////////////////////////////////////////////////////////////
// Fields
//////////////////////////////////////////////////////////////////////////

  ** Password storage (managed Fantom-side, no Rust involvement)
  const override PasswordStore passwords

  ** Connection to the rust-folio process
  private const AtomicRef connRef

  ** Process manager for the rust-folio subprocess
  private const AtomicRef processRef

  ** History implementation
  private const RustFolioHis hisImpl

  ** Backup implementation
  private const RustFolioBackup backupImpl

  ** Display string manager (Fantom-side cache)
  private const RustFolioDisMgr disMgr

  private RustFolioConn? conn() { (connRef.val as Unsafe)?.val }
  private RustFolioProcess? rustProcess() { (processRef.val as Unsafe)?.val }

  ** Internal accessor for RustFolioHis to reach the connection.
  internal RustFolioConn? connForHis() { conn }

  ** Internal accessor for RustFolioBackup to reach the connection.
  internal RustFolioConn? connForBackup() { conn }

//////////////////////////////////////////////////////////////////////////
// Storage Metadata
//////////////////////////////////////////////////////////////////////////

  ** Current persistent version.
  override Int curVer()
  {
    c := conn ?: throw ShutdownErr("$typeof.name is closed")
    return c.readCurVer
  }

  ** Flush mode — get or set.
  override Str flushMode
  {
    get
    {
      c := conn ?: throw ShutdownErr("$typeof.name is closed")
      return c.readFlushMode
    }
    set
    {
      c := conn ?: throw ShutdownErr("$typeof.name is closed")
      c.setFlushMode(it)
    }
  }

  ** Flush (redb fsync is automatic on commit).
  override Void flush()
  {
    c := conn ?: throw ShutdownErr("$typeof.name is closed")
    c.sendFlush
  }

  **
  ** Sync override.  When called with mgr == "dis" (from DisTest.syncDis and
  ** production code), re-read all records from Rust and recompute the
  ** disMacro dis cache.  This is the RustFolio equivalent of hxFolio's
  ** DisMgr.updateAll() triggered via HxFolio.sync(null, "dis").
  **
  override This sync(Duration? timeout := null, Str? mgr := null)
  {
    if (mgr == "dis")
    {
      c := conn
      if (c != null) disMgr.updateAll(c.readAll(Filter.has("id"), null))
    }
    return this
  }

//////////////////////////////////////////////////////////////////////////
// Subsystems
//////////////////////////////////////////////////////////////////////////

  ** Backup — backed by RustFolioBackup (redb snapshot + Fantom zip).
  override FolioBackup backup() { backupImpl }

  ** History — backed by redb via RustFolioHis.
  override FolioHis his() { hisImpl }

  ** File storage — not yet implemented.
  override FolioFile file()
  {
    throw UnsupportedErr("RustFolio.file not implemented")
  }

//////////////////////////////////////////////////////////////////////////
// Lifecycle
//////////////////////////////////////////////////////////////////////////

  **
  ** Close the database. Sends Close opcode to rust-folio, waits for
  ** the process to exit, then clears the connection.
  **
  override protected FolioFuture doCloseAsync()
  {
    // Swap out references so subsequent calls see a closed state
    c    := (connRef.getAndSet(null)    as Unsafe)?.val as RustFolioConn
    proc := (processRef.getAndSet(null) as Unsafe)?.val as RustFolioProcess

    try
    {
      if (c != null) c.close
    }
    catch (Err e) {}

    try
    {
      if (proc != null)
      {
        exitCode := proc.waitForExit
        if (exitCode == -1)
        {
          // Process did not exit cleanly — force kill
          proc.kill
        }
      }
    }
    catch (Err e) {}

    return FolioFuture.makeSync(CountFolioRes(0))
  }

//////////////////////////////////////////////////////////////////////////
// Reads
//////////////////////////////////////////////////////////////////////////

  override protected FolioRec? doReadRecById(Ref id)
  {
    c := conn ?: throw ShutdownErr("$typeof.name is closed")
    dict := c.readById(id)
    if (dict == null) return null
    dict = augmentHisTags(dict)
    disMgr.enrichRefs(dict)
    return RustFolioRec(dict)
  }

  override protected FolioFuture doReadByIds(Ref[] ids)
  {
    c := conn ?: throw ShutdownErr("$typeof.name is closed")
    dicts := c.readByIds(ids)
    recs   := Dict?[,]
    errMsg := ""
    dicts.each |d, i|
    {
      if (d != null)
      {
        d = augmentHisTags(d)
        disMgr.enrichRefs(d)
        recs.add(RustFolioRec(d).dict)
      }
      else
      {
        recs.add(null)
        if (errMsg.isEmpty) errMsg = ids[i].toStr
      }
    }
    return FolioFuture.makeSync(ReadFolioRes(errMsg, !errMsg.isEmpty, recs))
  }

  **
  ** Inject hisSize, hisStart, hisEnd into a record dict using the
  ** Rust-backed stats cache.  These are 'never' tags that cannot
  ** flow through a normal Diff — the folio implementation owns them.
  ** On cache miss a lazy HIS_STAT RPC is issued; subsequent reads use
  ** the cached value.  Timestamps are converted to the record's tz so
  ** that verifySame(r["hisStart"]->tz, tz) passes.
  **
  private Dict augmentHisTags(Dict dict)
  {
    if (!dict.has("his")) return dict

    id := dict["id"] as Ref
    if (id == null) return dict

    // Fetch from stat cache (O(1)) or lazy-load from Rust
    stat := hisImpl.statFor(id.id)
    if (stat == null) return dict

    tz := FolioUtil.hisTz(dict, false)
    if (tz == null) return dict

    first := DateTime.makeTicks(stat.firstTicks, TimeZone.utc).toTimeZone(tz)
    last  := DateTime.makeTicks(stat.lastTicks,  TimeZone.utc).toTimeZone(tz)
    map   := Str:Obj[:]
    dict.each |v, n| { map[n] = v }
    map["hisSize"]  = Number(stat.size)
    map["hisStart"] = first
    map["hisEnd"]   = last
    return Etc.makeDict(map)
  }

  override protected FolioFuture doReadAll(Filter filter, Dict? opts)
  {
    c    := conn ?: throw ShutdownErr("$typeof.name is closed")
    recs := c.readAll(filter, opts)
    return FolioFuture.makeSync(ReadFolioRes("", false, recs))
  }

  override protected Int doReadCount(Filter filter, Dict? opts)
  {
    c := conn ?: throw ShutdownErr("$typeof.name is closed")
    return c.readCount(filter, opts)
  }

  override protected Obj? doReadAllEachWhile(Filter filter, Dict? opts, |Dict->Obj?| f)
  {
    c    := conn ?: throw ShutdownErr("$typeof.name is closed")
    recs := c.readAll(filter, opts)
    return recs.eachWhile(f)
  }

//////////////////////////////////////////////////////////////////////////
// Commits
//////////////////////////////////////////////////////////////////////////

  override protected FolioFuture doCommitAllAsync(Diff[] diffs, Obj? cxInfo)
  {
    c     := conn ?: throw ShutdownErr("$typeof.name is closed")
    h     := hooks

    // Build pre-commit events and call preCommit (may throw to cancel)
    events := RustFolioCommitEvent[,]
    diffs.each |orig|
    {
      oldRec := orig.isAdd ? null : readById(orig.id, false)
      events.add(RustFolioCommitEvent(orig, oldRec, cxInfo))
    }
    events.each |e| { h.preCommit(e) }

    // Send diffs to Rust, get back (id, oldMod, newMod, oldRec, newRec) per diff
    results := c.commitAll(diffs)

    // Reconstruct completed Diffs from the result + original diff metadata
    completed := Diff[,]
    diffs.each |orig, i|
    {
      r := results[i]
      completed.add(Diff.makeAll(
        r.id,
        r.oldMod,
        r.oldRec,
        r.newMod,
        r.newRec,
        orig.changes,
        orig.flags))
    }

    // Update events with completed diffs and call postCommit
    completed.each |d, i| { events[i].completedDiff = d }
    events.each |e| { h.postCommit(e) }

    // Refresh the dis cache after every commit so that subsequent readById
    // calls return Refs with up-to-date disVal.  This mirrors hxFolio's
    // DisMgr.update(rec) + updateAll() pattern: every commit that may change
    // a record's dis immediately propagates to all disMacro dependents.
    disMgr.updateAll(c.readAll(Filter.has("id"), null))

    return FolioFuture.makeSync(CommitFolioRes(completed))
  }

}

**************************************************************************
** RustFolioCommitEvent
**************************************************************************

internal class RustFolioCommitEvent : FolioCommitEvent
{
  new make(Diff preDiff, Dict? oldRec, Obj? cxInfo)
  {
    this.preDiff  = preDiff
    this.oldRec   = oldRec
    this.cxInfo   = cxInfo
  }

  override Diff diff() { completedDiff ?: preDiff }
  override Dict? oldRec
  override Obj? cxInfo

  private Diff preDiff
  Diff? completedDiff
}
