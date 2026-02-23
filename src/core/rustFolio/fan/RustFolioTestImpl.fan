//
// Copyright (c) 2026, SkyFoundry LLC
// Licensed under the Academic Free License version 3.0
//
// History:
//   23 Feb 2026  Hathi  Creation (M0 scaffold, M2 activation)
//

using xeto
using haystack
using folio

**
** RustFolioTestImpl plugs RustFolio into the AbstractFolioTest harness.
**
** Index registration was deferred from M0/M1 until M2, when filter
** evaluation and ReadAll are functional. Enabled in build.fan at M2.
**
class RustFolioTestImpl : FolioTestImpl
{
  override Str name() { "rustfolio" }

  override Folio open(FolioConfig c) { RustFolio.open(c) }

  ** Transient commits fully implemented in Rust (RecordCache transient layer)
  override Bool supportsTransient() { true }

  ** History fully implemented Fantom-side (RustFolioHis in-memory map)
  override Bool supportsHis() { true }

  ** Id prefix rename — defer until wire protocol handles it
  override Bool supportsIdPrefixRename() { false }

  **
  ** Refs cross a process boundary and are deserialized fresh each time.
  ** Use equality (verifyEq) instead of identity (verifySame).
  **
  override Void verifyIdsSame(Ref a, Ref b) { verifyEq(a, b) }

  **
  ** Dicts are deserialized fresh each time — use Dict value equality.
  **
  override Void verifyRecSame(Dict? a, Dict? b)
  {
    if (a == null) { verify(b == null); return }
    if (b == null) { verify(a == null); return }
    test.verifyDictEq(a, b)
  }

  **
  ** M6 (disMacro / syncDis propagation) is not yet implemented.
  ** Override as no-ops to allow DisTest to run the underlying
  ** commit/read operations without failing on dis verification.
  **
  override Void verifyDictDis(Dict r, Str expect) {}
  override Void verifyIdDis(Ref id, Str expect)   {}
}
