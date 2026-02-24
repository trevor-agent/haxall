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
**   - FolioWatch and watch lifecycle (including rec cache for ticks/watchCount)
**   - Pre/post-commit hook dispatch (hook identity required by tests)
**   - Backup and file storage
**   - Transient registry + reconnect logic
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
    connRef    = AtomicRef(Unsafe(conn))
    processRef = AtomicRef(Unsafe(proc))

    // History implementation (see RustFolioHis)
    hisImpl = RustFolioHis(this)

    // Backup implementation (see RustFolioBackup)
    backupImpl = RustFolioBackup(this)

    // File storage — local filesystem delegation via LocalFolioFile
    fileImpl = LocalFolioFile(this)

    // Display string manager — compute initial dis cache on open.
    disMgr = RustFolioDisMgr()
    disMgr.updateAll(conn.readAll(Filter.has("id"), null))

    // Seed lastKnownVerRef so Q3 curVer delta detection has a baseline.
    lastKnownVerRef.val = conn.readCurVer
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

  ** File storage — local filesystem via LocalFolioFile
  private const LocalFolioFile fileImpl

  ** Display string manager (Fantom-side cache)
  private const RustFolioDisMgr disMgr

  **
  ** Canonical FolioRec cache — one RustFolioRec per record id.
  ** Ensures stable ticks and watchCount across repeated readRecById calls.
  ** Mirrors hxFolio's shared Rec objects.  Access is safe because the folio
  ** actor is single-threaded.
  **
  private const ConcurrentMap recCache := ConcurrentMap()

  **
  ** Transient registry — maps record id to its current effective transient
  ** tag overlay.  Updated after each successful transient commit.  Replayed
  ** to the new Rust process after reconnect.
  **
  ** Transient commits are ALWAYS updates to existing persistent records
  ** (Diff.transient + Diff.add is invalid per the Diff API).
  **
  private const AtomicRef transientRegistryRef := AtomicRef(Unsafe(Str:TransientEntry[:]))

  ** True while doReconnect() is executing (used by status() and doReadRecById).
  private const AtomicBool reconnectingRef := AtomicBool(false)

  ** Last curVer Fantom successfully confirmed from Rust.
  ** Seeded at open() and incremented after each successful persistent commitAll.
  ** Used in doReconnect() for Q3 phantom-commit detection.
  **
  private const AtomicInt lastKnownVerRef := AtomicInt(0)

  private RustFolioConn? conn() { (connRef.val as Unsafe)?.val }
  private RustFolioProcess? rustProcess() { (processRef.val as Unsafe)?.val }

  ** Internal accessor for RustFolioHis to reach the connection.
  internal RustFolioConn? connForHis() { conn }

  ** Internal accessor for RustFolioBackup to reach the connection.
  internal RustFolioConn? connForBackup() { conn }

//////////////////////////////////////////////////////////////////////////
// Status
//////////////////////////////////////////////////////////////////////////

  **
  ** Operator-visible connection state:
  **   "connected"    — normal operation
  **   "reconnecting" — in the process of kill + respawn
  **   "error"        — all reconnect attempts failed; folio is unusable
  **
  Str status()
  {
    if (reconnectingRef.val)   return "reconnecting"
    if (connRef.val != null)   return "connected"
    return "error"
  }

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
  ** disMacro dis cache.
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

  ** File storage — local filesystem via LocalFolioFile.
  override FolioFile file() { fileImpl }

//////////////////////////////////////////////////////////////////////////
// Lifecycle
//////////////////////////////////////////////////////////////////////////

  override protected FolioFuture doCloseAsync()
  {
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
        if (exitCode == -1) proc.kill
      }
    }
    catch (Err e) {}

    return FolioFuture.makeSync(CountFolioRes(0))
  }

//////////////////////////////////////////////////////////////////////////
// Reconnect
//////////////////////////////////////////////////////////////////////////

  private static const Int maxReconnectAttempts := 3
  private static const Duration reconnectDelay   := 1sec

  **
  ** Reconnect after a detected process crash.  Runs synchronously in the
  ** folio actor (single-threaded) — all pending actor messages queue while
  ** reconnect is in progress.
  **
  ** Protocol:
  **  1. Kill the old process (best effort — it may already be dead).
  **  2. Spawn a fresh process and connect.
  **  3. Q3: detect phantom commit via curVer delta.
  **  4. Install new conn + process atomically.
  **  5. Replay transient registry.
  **  6. Recompute dis cache (after transient replay so transient records
  **     are visible to the dis computation).
  **  7. Clear his stats cache (lazy re-population).
  **
  ** If all attempts fail, nulls connRef and throws ShutdownErr.
  **
  private Void doReconnect()
  {
    reconnectingRef.val = true
    try
    {
      Int attempt := 0
      Err? lastErr := null
      while (attempt < maxReconnectAttempts)
      {
        attempt++
        try
        {
          doReconnectAttempt
          log.info("rust-folio reconnected successfully after $attempt attempt(s)")
          return
        }
        catch (Err e)
        {
          lastErr = e
          log.err("rust-folio reconnect attempt ${attempt}/${maxReconnectAttempts} failed", e)
          if (attempt < maxReconnectAttempts) Actor.sleep(reconnectDelay)
        }
      }
      // All attempts failed — mark as error state
      connRef.val = null
      throw ShutdownErr("rust-folio reconnect failed after ${maxReconnectAttempts} attempts: ${lastErr?.msg}")
    }
    finally
    {
      reconnectingRef.val = false
    }
  }

  private Void doReconnectAttempt()
  {
    // 1. Teardown: close old conn and kill old process (best-effort)
    oldConn := (connRef.getAndSet(null) as Unsafe)?.val as RustFolioConn
    try { oldConn?.close } catch (Err e) {}

    oldProc := (processRef.getAndSet(null) as Unsafe)?.val as RustFolioProcess
    if (oldProc != null && oldProc.isAlive)
      try { oldProc.kill } catch (Err e) {}

    // 2. Spawn fresh process
    proc := RustFolioProcess(dir)
    port := proc.start(config)

    // 3. Connect
    newConn := RustFolioConn()
    newConn.connect(port)

    // 4. Q3 — curVer delta detection (phantom commit warning)
    postReconnectVer := newConn.readCurVer
    preVer := lastKnownVerRef.val
    if (postReconnectVer > preVer)
      log.warn("rust-folio reconnect detected phantom commit (curVer ${preVer} → ${postReconnectVer})")
    lastKnownVerRef.val = postReconnectVer

    // 5. Install new conn + process
    processRef.val = Unsafe(proc)
    connRef.val    = Unsafe(newConn)

    // 6. Replay transient registry (must precede dis cache recompute)
    replayTransients(newConn)

    // 7. Recompute dis cache
    disMgr.updateAll(newConn.readAll(Filter.has("id"), null))

    // 8. Clear his stats cache — lazy re-population from new process
    hisImpl.clearStatsCache
  }

//////////////////////////////////////////////////////////////////////////
// Transient Registry
//////////////////////////////////////////////////////////////////////////

  private Str:TransientEntry transientRegistry()
  {
    ((Unsafe)transientRegistryRef.val).val
  }

  private Void setTransientRegistry(Str:TransientEntry r)
  {
    transientRegistryRef.val = Unsafe(r)
  }

  **
  ** Update the transient registry after a successful commit batch.
  ** Only called when at least one diff in the batch is transient.
  **
  private Void updateTransientRegistry(Diff[] diffs)
  {
    reg := transientRegistry.dup
    diffs.each |d|
    {
      id := d.id.id

      if (!d.isTransient)
      {
        // Persistent remove: clean up any transient overlay for this record
        if (d.isRemove) reg.remove(id)
        return
      }

      // Transient update (transient+add and transient+remove are both
      // invalid per Diff validation — only transient updates are possible)
      existing := reg[id]
      if (existing != null)
      {
        // Merge delta into existing effective tags
        merged := mergeTransientChanges(existing.tags, d.changes)
        if (merged.isEmpty) reg.remove(id)    // all transient tags removed
        else                reg[id] = TransientEntry(merged)
      }
      else
      {
        if (!d.changes.isEmpty) reg[id] = TransientEntry(d.changes)
      }
    }
    setTransientRegistry(reg)
  }

  **
  ** Apply a delta Dict onto an existing transient tag overlay.
  ** Tags whose value is None.val (Remove sentinel) are removed;
  ** all others are set.
  **
  private static Dict mergeTransientChanges(Dict existing, Dict delta)
  {
    result := Str:Obj?[:]
    existing.each |v, k| { result[k] = v }
    delta.each |v, k|
    {
      if (v === None.val) result.remove(k)
      else                result[k] = v
    }
    return Etc.makeDict(result)
  }

  **
  ** Replay all transient registry entries to the given connection.
  ** Called after a successful reconnect, before dis cache recompute.
  **
  private Void replayTransients(RustFolioConn c)
  {
    reg := transientRegistry
    if (reg.isEmpty) return

    replayDiffs := Diff[,]
    reg.each |TransientEntry entry, Str id|
    {
      // Fetch the persistent record from the new process
      rec := c.readById(Ref(id))
      if (rec == null)
      {
        // Persistent record gone — remove stale entry from registry
        return
      }
      // Build a transient update diff for the current effective overlay
      replayDiffs.add(Diff(rec, entry.tags, Diff.transient))
    }

    if (!replayDiffs.isEmpty)
    {
      try
      {
        c.commitAll(replayDiffs)
        log.info("rust-folio replayed ${replayDiffs.size} transient overlay(s)")
      }
      catch (Err e)
      {
        log.err("rust-folio transient replay failed", e)
        // Non-fatal — reconnect still succeeds; transient state will be stale
      }
    }

    // Clean up any entries whose records no longer exist
    stale := reg.keys.findAll |id| { c.readById(Ref(id)) == null }
    if (!stale.isEmpty)
    {
      fresh := reg.dup
      stale.each |id| { fresh.remove(id) }
      setTransientRegistry(fresh)
    }
  }

//////////////////////////////////////////////////////////////////////////
// Rec Cache
//////////////////////////////////////////////////////////////////////////

  **
  ** Get the canonical RustFolioRec for the given id, refreshing its dict.
  ** Creates and caches a new instance if this is the first read.
  **
  private RustFolioRec getOrCreateRec(Str id, Dict dict)
  {
    existing := recCache.get(id) as RustFolioRec
    if (existing != null)
    {
      existing.refreshDict(dict)
      return existing
    }
    rec := RustFolioRec(dict)
    recCache.set(id, rec)
    return rec
  }

//////////////////////////////////////////////////////////////////////////
// Reads
//////////////////////////////////////////////////////////////////////////

  override protected FolioRec? doReadRecById(Ref id)
  {
    c := conn
    if (c == null)
    {
      // During reconnect window: return stale cached rec to keep watch polls alive.
      // After close (not reconnecting): throw.
      if (reconnectingRef.val) return recCache.get(id.id) as RustFolioRec
      throw ShutdownErr("$typeof.name is closed")
    }

    try
    {
      return doReadRecByIdFrom(c, id)
    }
    catch (IOErr e)
    {
      doReconnect
      c2 := conn ?: throw e
      return doReadRecByIdFrom(c2, id)
    }
  }

  private FolioRec? doReadRecByIdFrom(RustFolioConn c, Ref id)
  {
    dict := c.readById(id)
    if (dict == null)
    {
      recCache.remove(id.id)
      return null
    }
    dict = augmentHisTags(dict)
    disMgr.enrichRefs(dict)
    return getOrCreateRec(id.id, dict)
  }

  override protected FolioFuture doReadByIds(Ref[] ids)
  {
    c := conn ?: throw ShutdownErr("$typeof.name is closed")
    try
    {
      return doReadByIdsFrom(c, ids)
    }
    catch (IOErr e)
    {
      doReconnect
      c2 := conn ?: throw e
      return doReadByIdsFrom(c2, ids)
    }
  }

  private FolioFuture doReadByIdsFrom(RustFolioConn c, Ref[] ids)
  {
    dicts  := c.readByIds(ids)
    recs   := Dict?[,]
    errMsg := ""
    dicts.each |d, i|
    {
      if (d != null)
      {
        d = augmentHisTags(d)
        disMgr.enrichRefs(d)
        recs.add(d)
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
  ** Rust-backed stats cache.
  **
  private Dict augmentHisTags(Dict dict)
  {
    if (!dict.has("his")) return dict

    id := dict["id"] as Ref
    if (id == null) return dict

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
    c := conn ?: throw ShutdownErr("$typeof.name is closed")
    try
    {
      recs := c.readAll(filter, opts)
      return FolioFuture.makeSync(ReadFolioRes("", false, recs))
    }
    catch (IOErr e)
    {
      doReconnect
      c2 := conn ?: throw e
      recs := c2.readAll(filter, opts)
      return FolioFuture.makeSync(ReadFolioRes("", false, recs))
    }
  }

  override protected Int doReadCount(Filter filter, Dict? opts)
  {
    c := conn ?: throw ShutdownErr("$typeof.name is closed")
    try
    {
      return c.readCount(filter, opts)
    }
    catch (IOErr e)
    {
      doReconnect
      c2 := conn ?: throw e
      return c2.readCount(filter, opts)
    }
  }

  override protected Obj? doReadAllEachWhile(Filter filter, Dict? opts, |Dict->Obj?| f)
  {
    c := conn ?: throw ShutdownErr("$typeof.name is closed")
    try
    {
      recs := c.readAll(filter, opts)
      return recs.eachWhile(f)
    }
    catch (IOErr e)
    {
      doReconnect
      c2 := conn ?: throw e
      recs := c2.readAll(filter, opts)
      return recs.eachWhile(f)
    }
  }

//////////////////////////////////////////////////////////////////////////
// Commits
//////////////////////////////////////////////////////////////////////////

  override protected FolioFuture doCommitAllAsync(Diff[] diffs, Obj? cxInfo)
  {
    c     := conn ?: throw ShutdownErr("$typeof.name is closed")
    h     := hooks
    hasTransient := diffs.any |d| { d.isTransient }

    // Build pre-commit events and call preCommit (may throw to cancel)
    events := RustFolioCommitEvent[,]
    diffs.each |orig|
    {
      oldRec := orig.isAdd ? null : readById(orig.id, false)
      events.add(RustFolioCommitEvent(orig, oldRec, cxInfo))
    }
    events.each |e| { h.preCommit(e) }

    // Send diffs to Rust — catch IOErr for reconnect handling.
    // Reconnect does NOT retry the commit (writes are not idempotent).
    RustCommitResult[] results := [,]
    try
    {
      results = c.commitAll(diffs)
    }
    catch (IOErr e)
    {
      doReconnect
      throw e  // propagate original IOErr — caller handles recovery
    }

    // Reconstruct completed Diffs from results + original diff metadata
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

    // Update the Fantom-side rec cache and transient registry
    updateRecCacheAfterCommit(completed)
    if (hasTransient) updateTransientRegistry(diffs)

    // Refresh the dis cache after every commit
    disMgr.updateAll(c.readAll(Filter.has("id"), null))

    return FolioFuture.makeSync(CommitFolioRes(completed))
  }

  **
  ** Update the rec cache after a successful commit batch.
  ** For adds and updates: stamp the cached rec with the new dict and nowTicks.
  ** For removes: evict from cache.
  **
  private Void updateRecCacheAfterCommit(Diff[] completed)
  {
    // Track whether any non-transient commit happened (for lastKnownVerRef)
    anyPersistent := false

    completed.each |d|
    {
      id := d.id.id

      if (d.isRemove)
      {
        recCache.remove(id)
        return
      }

      newDict := d.newRec
      if (newDict == null) return

      existing := recCache.get(id) as RustFolioRec
      if (existing != null)
        existing.updateOnCommit(newDict)
      else
      {
        newRec := RustFolioRec(newDict)
        newRec.updateOnCommit(newDict)
        recCache.set(id, newRec)
      }

      if (!d.isTransient) anyPersistent = true
    }

    // Advance the last-known version for non-transient commits (Q3 detection)
    if (anyPersistent) lastKnownVerRef.val = lastKnownVerRef.val + 1
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

**************************************************************************
** TransientEntry
**************************************************************************

**
** One entry in the transient registry — the current effective transient tag
** overlay for a single persistent record.
**
** Note: transient + add and transient + remove are both invalid per the Diff
** API, so all registry entries represent overlay updates to persistent records.
**
const class TransientEntry
{
  new make(Dict tags) { this.tags = tags }

  ** Current effective transient tag overlay (the full set of tags to replay).
  const Dict tags
}
