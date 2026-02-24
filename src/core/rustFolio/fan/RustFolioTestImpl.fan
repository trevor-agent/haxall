//
// Copyright (c) 2026, SkyFoundry LLC
// Licensed under the Academic Free License version 3.0
//
// History:
//   23 Feb 2026  Hathi  Creation
//

using xeto
using haystack
using folio

**
** RustFolioTestImpl plugs RustFolio into the AbstractFolioTest harness.
**
class RustFolioTestImpl : FolioTestImpl
{
  override Str name() { "rustfolio" }

  override Folio open(FolioConfig c) { RustFolio.open(c) }

  ** Transient commits fully implemented in Rust (RecordCache transient layer)
  override Bool supportsTransient() { true }

  ** History fully implemented (RustFolioHis, backed by redb)
  override Bool supportsHis() { true }

  ** Id prefix rename implemented in Server::open() via atomic redb rewrite.
  override Bool supportsIdPrefixRename() { true }

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
  ** verifyDictDis and verifyIdDis use the default FolioTestImpl behaviour:
  **   verifyEq(r.dis, expect) + verifyEq(r.id.dis, expect)
  ** RustFolioDisMgr.enrichRefs() sets Ref.disVal on all Refs returned by
  ** readById, so both Dict.dis (via Etc.dictToDis + Macro.refToDis) and
  ** Ref.dis resolve correctly for disMacro records.
  **
}
