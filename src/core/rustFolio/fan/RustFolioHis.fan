//
// Copyright (c) 2026, SkyFoundry LLC
// Licensed under the Academic Free License version 3.0
//
// History:
//   23 Feb 2026  Hathi  Creation (M5)
//

using concurrent
using xeto
using haystack
using folio

**
** RustFolioHis implements FolioHis entirely on the Fantom side using an
** in-memory map, matching hxFolio's HisMgr design.
**
** Design notes:
**   - History items are stored as HisItem[] per record id (full id string).
**   - Tz and unit are applied on-the-fly during reads (not baked into stored items).
**     This means tz/unit config changes on the record are reflected immediately
**     in subsequent reads without needing to re-write history data.
**   - hisSize, hisStart, hisEnd tags are injected into the record dict by
**     RustFolio.augmentHisTags() at read time (they are 'never' tags that
**     cannot be set via a normal Diff).
**   - Thread safety: AtomicRef<Unsafe<Str:HisItem[]>> — all operations are
**     lightweight and the Fantom-side folio actor provides logical serialization.
**
const class RustFolioHis : FolioHis
{
  new make(RustFolio folio) { this.folio = folio }

  private const RustFolio folio
  private const AtomicRef hisRef := AtomicRef(Unsafe(Str:HisItem[][:]))

  ** Return items stored for the given (absolute) record id string.
  HisItem[] itemsFor(Str id) { data[id] ?: HisItem#.emptyList }

  private Str:HisItem[] data() { ((Unsafe)hisRef.val).val }
  private Void setData(Str:HisItem[] d) { hisRef.val = Unsafe(d) }

//////////////////////////////////////////////////////////////////////////
// Read
//////////////////////////////////////////////////////////////////////////

  override Void read(Ref id, Span? span, Dict? opts, |HisItem| f)
  {
    if (opts == null) opts = Etc.dict0

    // resolve current record and validate his config
    rec := folio.readById(id, false)
    if (rec == null) throw HisConfigErr(Etc.emptyDict, "Unknown rec: $id.toCode")
    validateRead(rec)

    // resolve config: tz, kind, unit
    tz   := FolioUtil.hisTz(rec)
    kind := FolioUtil.hisKind(rec)
    unit := FolioUtil.hisUnit(rec)

    // get stored items and yield with config applied
    items := itemsFor(id.id)

    if (span == null)
    {
      items.each |item| { f(applyConfig(item, tz, unit)) }
    }
    else
    {
      // SkySpark semantics: include 1 item before span.start and 2 items after span.end
      HisItem? prev := null
      Int after := 0
      items.each |item|
      {
        normalized := applyConfig(item, tz, unit)
        if (normalized.ts < span.start)
        {
          prev = normalized
        }
        else if (normalized.ts >= span.end)
        {
          if (after < 2) { f(normalized); after++ }
        }
        else
        {
          if (prev != null) { f(prev); prev = null }
          f(normalized)
        }
      }
    }
  }

//////////////////////////////////////////////////////////////////////////
// Write
//////////////////////////////////////////////////////////////////////////

  override FolioFuture write(Ref id, HisItem[] items, Dict? opts := null)
  {
    if (opts == null) opts = Etc.dict0

    // resolve current record and validate his config
    rec := folio.readById(id, false)
    if (rec == null) throw HisConfigErr(Etc.emptyDict, "Unknown rec: $id.toCode")

    // empty write short-circuit
    if (items.isEmpty) return FolioFuture.makeSync(HisWriteFolioRes.empty)

    // force unitSet: items written to a record with a unit always get the unit
    opts = Etc.dictSet(opts, "unitSet", Marker.val)

    // validate config and normalize items (sort, tz/kind/unit check, ts precision)
    // FolioUtil.hisWriteCheck does: point+his check, aux+trash check, tz check,
    // kind check, unit check, timestamp normalization, dedup
    normalized := FolioUtil.hisWriteCheck(rec, items, opts)

    // merge with existing stored items (sorted merge with overwrite/remove semantics)
    cur    := itemsFor(id.id)
    merged := FolioUtil.hisWriteMerge(cur, normalized)

    // update in-memory store
    newData := data.dup.set(id.id, merged)
    setData(newData)

    // build result dict: count + span covering the written items
    span   := Span.makeAbs(normalized.first.ts, normalized.last.ts)
    result := Etc.makeDict(["count": Number(normalized.size), "span": span])

    // dispatch postHisWrite hook with cxInfo from current thread context
    cxInfo := FolioContext.curFolio(false)?.commitInfo
    folio.hooks.postHisWrite(RustFolioHisEvent(rec, result, cxInfo))

    return FolioFuture.makeSync(HisWriteFolioRes(result))
  }

//////////////////////////////////////////////////////////////////////////
// Helpers
//////////////////////////////////////////////////////////////////////////

  **
  ** Validate that a record is a readable his point.
  ** Mirrors hxFolio HisMgr.read checks.
  **
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
  ** Apply the record's current tz and unit to a stored item.
  ** This is done on-the-fly so that tz/unit config changes take
  ** effect immediately on subsequent reads.
  **
  private static HisItem applyConfig(HisItem item, TimeZone tz, Unit? unit)
  {
    ts  := item.ts.toTimeZone(tz)
    val := item.val

    // Apply unit to unitless Number values when the record has a unit.
    // If the item already has a unit it was stored with unitSet and is correct.
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
