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
** Watch tracking is local to the Fantom side since watches are a Fantom-level concern.
** Refs cross a process boundary — each deserialization produces a new Dict instance,
** so identity-based checks (verifySame) must use equality instead.
**
const class RustFolioRec : FolioRec
{
  new make(Dict dict) { this.dictRef = dict }

  override Dict dict() { dictRef }
  private const Dict dictRef

  override Int ticks()            { ticksRef.val }
  override Int watchCount()       { watchRef.val }
  override Int watchIncrement()   { watchRef.incrementAndGet }
  override Int watchDecrement()   { watchRef.decrementAndGet }

  private const AtomicInt ticksRef := AtomicInt(Duration.nowTicks)
  private const AtomicInt watchRef := AtomicInt(0)
}
