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
** RustFolioRec wraps a Dict for the FolioRec mixin.
**
** Design: one canonical instance per record id, held in RustFolio.recCache.
** The dict is mutable in-place (AtomicRef) so that reads can refresh the
** content without creating a new instance.  ticks and watchCount are stable
** across reads — they are only updated when the record is actually committed
** (updateOnCommit) or added to a watch (watchIncrement/watchDecrement).
**
** This mirrors hxFolio's Rec design and fixes two watch bugs that arise when
** a new RustFolioRec is constructed per readRecById call:
**   BUG-1: ticks = nowTicks at construction → every record looks "changed" on
**          every watch poll, even when nothing changed.
**   BUG-2: watchRef = AtomicInt(0) per instance → watchCount is always 0,
**          watchIncrement/Decrement have no lasting effect.
**
const class RustFolioRec : FolioRec
{
  **
  ** Make a RustFolioRec.  ticks starts at 1 (unmodified baseline) so that
  ** watch polls do not see the record as "changed" until updateOnCommit() is
  ** called.  Use make() for newly-read records; call updateOnCommit() immediately
  ** after if the record was just committed.
  **
  new make(Dict dict) { dictRef = AtomicRef(dict) }

  ** Update the dict in-place without changing ticks or watchCount.
  ** Called from doReadRecById after fetching the latest dict from Rust.
  Void refreshDict(Dict d) { dictRef.val = d }

  **
  ** Update both dict and ticks.  Called after a successful commit so that
  ** any open watch poll will see this record as changed.
  **
  Void updateOnCommit(Dict d)
  {
    dictRef.val  = d
    ticksRef.val = Duration.nowTicks
  }

  override Dict dict() { dictRef.val }

  ** Ticks of last persistent or transient change (1 = never modified via this instance).
  override Int ticks() { ticksRef.val }

  ** Number of active watchers on this record.
  override Int watchCount()     { watchRef.val }

  ** Increment watch count; returns new count.
  override Int watchIncrement() { watchRef.incrementAndGet }

  ** Decrement watch count; returns new count.
  override Int watchDecrement() { watchRef.decrementAndGet }

  private const AtomicRef dictRef                  // Dict — updated on read + commit
  private const AtomicInt ticksRef := AtomicInt(1) // 1 = baseline (never "now")
  private const AtomicInt watchRef := AtomicInt(0)
}
