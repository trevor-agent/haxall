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
** RustFolioHis implements FolioHis by delegating to the Rust process for
** persistent storage (redb HISTORY + HISTORY_META tables).
**
** Design:
**   - FolioUtil.hisWriteCheck is still called Fantom-side for full validation
**     and normalization (sort, dedup, tz, kind, unit).  The cleaned item list
**     is then forwarded to Rust via RustFolioConn.hisWrite().
**   - Tz and unit are applied on-the-fly during reads (Fantom side) so that
**     config changes on the record take effect immediately without rewriting
**     stored data.  Rust returns raw UTC-ticks + val bytes; Fantom applies
**     applyConfig() before yielding to the caller.
**   - Span boundary semantics (1 item before span.start, up to 2 after
**     span.end) are handled Rust-side for efficient log-n seeks.
**   - augmentHisTags (hisSize/hisStart/hisEnd) is served from a lightweight
**     Fantom-side stats cache (RustHisStat per point) that is updated after
**     every hisWrite and lazily populated from Rust on first access.
**
const class RustFolioHis : FolioHis
{
  new make(RustFolio folio) { this.folio = folio }

  private const RustFolio folio

  ** Lightweight stats cache: absolute id string → RustHisStat.
  ** Updated after every hisWrite; lazily populated via hisStat RPC on
  ** first augmentHisTags access for a point that survived a restart.
  private const AtomicRef statRef := AtomicRef(Unsafe(Str:RustHisStat[:]))

  private Str:RustHisStat statCache() { ((Unsafe)statRef.val).val }
  private Void setStatCache(Str:RustHisStat c) { statRef.val = Unsafe(c) }

  **
  ** Return cached stats for the given point id.
  ** On cache miss (e.g. after restart), performs a lazy HIS_STAT RPC.
  ** Returns null if the point has no stored history.
  **
  RustHisStat? statFor(Str id)
  {
    cached := statCache[id]
    if (cached != null) return cached.size == 0 ? null : cached

    // Cache miss — ask Rust (lazy load after restart)
    c := folio.connForHis
    if (c == null) return null
    stat := c.hisStat(Ref(id))
    // Cache result (even size==0 so we don't re-query)
    setStatCache(statCache.dup.set(id, stat))
    return stat.size == 0 ? null : stat
  }

//////////////////////////////////////////////////////////////////////////
// Read
//////////////////////////////////////////////////////////////////////////

  override Void read(Ref id, Span? span, Dict? opts, |HisItem| f)
  {
    if (opts == null) opts = Etc.dict0

    // Resolve record and validate his config
    rec := folio.readById(id, false)
    if (rec == null) throw HisConfigErr(Etc.emptyDict, "Unknown rec: $id.toCode")
    validateRead(rec)

    // Resolve config: tz, unit
    tz   := FolioUtil.hisTz(rec)
    unit := FolioUtil.hisUnit(rec)

    // Read from Rust (span semantics applied Rust-side)
    c := folio.connForHis ?: throw ShutdownErr("RustFolio is closed")
    items := c.hisRead(id, span)

    // Apply tz/unit and yield
    items.each |item| { f(applyConfig(item, tz, unit)) }
  }

//////////////////////////////////////////////////////////////////////////
// Write
//////////////////////////////////////////////////////////////////////////

  override FolioFuture write(Ref id, HisItem[] items, Dict? opts := null)
  {
    if (opts == null) opts = Etc.dict0

    // Resolve record and validate his config
    rec := folio.readById(id, false)
    if (rec == null) throw HisConfigErr(Etc.emptyDict, "Unknown rec: $id.toCode")

    // Empty write short-circuit
    if (items.isEmpty) return FolioFuture.makeSync(HisWriteFolioRes.empty)

    // Force unitSet: items written to a record with a unit always get the unit
    opts = Etc.dictSet(opts, "unitSet", Marker.val)

    // Validate, sort, dedup, normalize (Fantom-side)
    normalized := FolioUtil.hisWriteCheck(rec, items, opts)

    // Persist to Rust
    c := folio.connForHis ?: throw ShutdownErr("RustFolio is closed")
    stat := c.hisWrite(id, normalized)

    // Update stats cache
    setStatCache(statCache.dup.set(id.id, stat))

    // Build result dict
    span   := Span.makeAbs(normalized.first.ts, normalized.last.ts)
    result := Etc.makeDict(["count": Number(normalized.size), "span": span])

    // Dispatch postHisWrite hook
    cxInfo := FolioContext.curFolio(false)?.commitInfo
    folio.hooks.postHisWrite(RustFolioHisEvent(rec, result, cxInfo))

    return FolioFuture.makeSync(HisWriteFolioRes(result))
  }

//////////////////////////////////////////////////////////////////////////
// Helpers
//////////////////////////////////////////////////////////////////////////

  private static Void validateRead(Dict rec)
  {
    if (rec.missing("point") || rec.missing("his"))
      throw HisConfigErr(rec, "Not tagged as his point")
    if (rec.has("aux"))
      throw HisConfigErr(rec, "Cannot read aux point")
    if (rec.has("trash"))
      throw HisConfigErr(rec, "Cannot read from trash")
  }

  **
  ** Apply the record's current tz and unit to a raw item returned by Rust.
  ** Rust stores ticks + val without tz/unit; we apply them here so that
  ** config changes on the record take effect immediately.
  **
  private static HisItem applyConfig(HisItem item, TimeZone tz, Unit? unit)
  {
    ts  := item.ts.toTimeZone(tz)
    val := item.val

    if (val is Number && unit != null)
    {
      num := (Number)val
      if (num.unit == null)
        val = Number(num.toFloat, unit)
    }

    return HisItem(ts, val)
  }

}

**************************************************************************
** RustFolioHisEvent
**************************************************************************

internal class RustFolioHisEvent : FolioHisEvent
{
  new make(Dict r, Dict res, Obj? cx)
  {
    this.rec    = r
    this.result = res
    this.cxInfo = cx
  }

  override const Dict rec
  override const Dict result
  override const Obj? cxInfo
}
