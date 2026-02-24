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
** RustFolioDisMgr computes and caches display strings for all records.
**
** Design mirrors hxFolio's DisMgr, adapted for the decoupled Rust process:
**   - hxFolio uses shared in-memory Rec objects with a single mutable Ref.disVal
**     per record; updating the Ref immediately affects every dict that holds it
**   - RustFolioDisMgr maintains a Str:Str id→dis cache in an AtomicRef
**   - updateAll() accepts all record dicts, computes dis recursively, updates cache
**   - enrichRefs() injects cached dis values into all Ref-typed tags in a Dict,
**     making both Dict.dis and Ref.dis correct for disMacro records on readById
**
** Thread safety: the AtomicRef holding the cache map is swapped atomically on
** every updateAll().  enrichRefs() takes a snapshot of the cache once and uses
** it for the whole dict walk — no explicit locking needed.
**
const class RustFolioDisMgr
{

//////////////////////////////////////////////////////////////////////////
// Cache Access
//////////////////////////////////////////////////////////////////////////

  ** Snapshot of the current id → computedDis cache.
  Str:Str cache() { ((Unsafe)cacheRef.val).val }

  ** Return the cached dis for the given record id, or the raw id string as fallback.
  Str getDis(Str idStr) { cache[idStr] ?: idStr }

//////////////////////////////////////////////////////////////////////////
// Update
//////////////////////////////////////////////////////////////////////////

  **
  ** Read all record dicts, compute dis for each (recursively resolving
  ** disMacro patterns), and replace the in-memory cache atomically.
  **
  ** Called on folio open, reconnect, and folio.sync(null, "dis").
  ** Use updateDirty() for incremental updates after normal commits.
  **
  Void updateAll(Dict[] allDicts)
  {
    // Build id → dict map (absolute id string → dict)
    Str:Dict idToDict := Str:Dict[:]
    allDicts.each |d|
    {
      id := d["id"] as Ref
      if (id != null) idToDict[id.id] = d
    }

    // Compute dis for every record; recursive with anti-cycle guard
    Str:Str newCache := Str:Str[:]
    idToDict.keys.each |idStr| { toDis(idStr, idToDict, newCache) }

    cacheRef.val = Unsafe(newCache)
  }

  **
  ** Incrementally update the dis cache after a normal commit.
  **
  ** Instead of reading all records (O(n)), this updates only the dirty set:
  **   - committedDicts: new record dicts from the completed commit batch
  **     (sourced from Diff.newRec — no extra RPC needed)
  **   - removedIds:    record ids that were removed in this commit
  **   - macroDicts:    all records in the DB with a disMacro tag
  **     (one filtered readAll query — typically a small set)
  **
  ** Non-dirty cache entries remain valid because:
  **   1. Their records were not touched by this commit.
  **   2. The toDis() memoization guard short-circuits on any id still
  **      present in the cache copy, so cross-reference resolution for
  **      macro records that point at non-dirty records is free.
  **
  ** The committed dicts overwrite any stale disMacro result for records
  ** that are both committed and have disMacro (last-write-wins in idToDict).
  **
  Void updateDirty(Dict[] committedDicts, Str[] removedIds, Dict[] macroDicts)
  {
    // Build id → dict for cross-reference resolution during macro eval.
    // macroDicts first, then committedDicts — so committed state wins if
    // a record appears in both (it was a disMacro record that just changed).
    Str:Dict idToDict := Str:Dict[:]
    macroDicts.each    |d| { id := (d["id"] as Ref)?.id; if (id != null) idToDict[id] = d }
    committedDicts.each |d| { id := (d["id"] as Ref)?.id; if (id != null) idToDict[id] = d }

    // Start with a copy of the current cache.  Non-dirty entries stay valid.
    Str:Str newCache := cache.dup

    // Evict removed records from the cache entirely.
    removedIds.each |id| { newCache.remove(id) }

    // Clear dirty IDs so toDis() recomputes them rather than short-circuiting.
    dirtyIds := Str[,]
    committedDicts.each |d| { id := (d["id"] as Ref)?.id; if (id != null) dirtyIds.add(id) }
    macroDicts.each    |d| { id := (d["id"] as Ref)?.id; if (id != null) dirtyIds.add(id) }
    dirtyIds.each |id| { newCache.remove(id) }

    // Recompute dis for every dirty ID.
    dirtyIds.each |idStr| { toDis(idStr, idToDict, newCache) }

    cacheRef.val = Unsafe(newCache)
  }

//////////////////////////////////////////////////////////////////////////
// Enrichment
//////////////////////////////////////////////////////////////////////////

  **
  ** Walk all tags in dict.  For any Ref-valued tag (including the synthetic
  ** "id" tag and cross-record refs like "aRef"), set Ref.disVal from the cache.
  **
  ** This is what makes Dict.dis work correctly for disMacro records:
  **   Etc.dictToDis evaluates "disMacro" via a standard Macro, which calls
  **   refToDis → Ref.dis → Ref.disVal.  Setting disVal here means that
  **   subsequent Dict.dis calls on the returned record return the fully
  **   resolved dis string.
  **
  Void enrichRefs(Dict dict)
  {
    c := cache
    dict.each |val, name|
    {
      if (val is Ref)
      {
        ref := (Ref)val
        dis := c[ref.id]
        if (dis != null) ref.disVal = dis
      }
    }
  }

//////////////////////////////////////////////////////////////////////////
// Recursive Computation
//////////////////////////////////////////////////////////////////////////

  **
  ** Memoized recursive dis computation.
  **
  ** Anti-cycle guard: before recursing, the raw id string is stored as the
  ** default so that circular disMacro references (record points to itself)
  ** resolve to the id string rather than looping forever — matching
  ** hxFolio's DisMgr.toDis behaviour exactly.
  **
  Str toDis(Str idStr, Str:Dict idToDict, Str:Str newCache)
  {
    // Memoized?
    x := newCache[idStr]
    if (x != null) return x

    // Anti-cycle: store the bare id as fallback before recursing
    newCache[idStr] = idStr

    // Resolve
    dict := idToDict[idStr]
    if (dict != null)
    {
      dis := computeDis(dict, idToDict, newCache)
      newCache[idStr] = dis
      return dis
    }

    return idStr
  }

  **
  ** Compute the dis string for a single record dict.
  ** For disMacro records this evaluates the macro pattern recursively.
  ** For plain records this returns Dict.dis (dis tag, name tag, or id string).
  **
  private Str computeDis(Dict dict, Str:Dict idToDict, Str:Str newCache)
  {
    disMacro := dict["disMacro"] as Str
    if (disMacro != null)
      return RustFolioMacro(disMacro, dict, idToDict, newCache, this).apply
    return dict.dis
  }

//////////////////////////////////////////////////////////////////////////
// Fields
//////////////////////////////////////////////////////////////////////////

  private const AtomicRef cacheRef := AtomicRef(Unsafe(Str:Str[:]))

}

**************************************************************************
** RustFolioMacro
**************************************************************************

**
** RustFolioMacro extends Macro to resolve Ref values through the DisMgr
** recursive cache during an updateAll() sweep.
**
** Standard Macro.refToDis returns Ref.dis = Ref.disVal ?: Ref.id.  During
** updateAll() the Ref objects deserialized from Rust do not yet have disVal
** set (that happens in enrichRefs after the sweep), so we must resolve via
** the in-progress newCache instead.
**
** This matches hxFolio's DisMgrMacro, which calls mgr.toDis(cache, ref) for
** the same reason (the DisMgr actor sets disVal only after computing the dis,
** so recursive lookups must go through the cache, not the Ref).
**
internal class RustFolioMacro : Macro
{
  new make(Str pattern, Dict scope,
           Str:Dict idToDict, Str:Str newCache, RustFolioDisMgr mgr)
    : super(pattern, scope)
  {
    this.idToDict = idToDict
    this.newCache = newCache
    this.mgr      = mgr
  }

  override Str refToDis(Ref ref) { mgr.toDis(ref.id, idToDict, newCache) }

  private Str:Dict        idToDict
  private Str:Str         newCache
  private RustFolioDisMgr mgr
}
