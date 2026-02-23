//
// Copyright (c) 2026, SkyFoundry LLC
// Licensed under the Academic Free License version 3.0
//
// History:
//   23 Feb 2026  Hathi  Creation (M0 scaffold)
//

using xeto
using haystack
using folio

**
** RustFolioTestImpl plugs RustFolio into the AbstractFolioTest harness.
**
** NOTE: Index registration ("testFolio.impl") is deferred to M2.
** See build.fan for rationale.
**
class RustFolioTestImpl : FolioTestImpl
{
  override Str name() { "rustfolio" }

  override Folio open(FolioConfig c) { RustFolio.open(c) }

  ** Transient commits not yet supported — enable at M3
  override Bool supportsTransient() { false }

  ** History not yet supported — enable at M5
  override Bool supportsHis() { false }

  ** Id prefix rename not yet supported — enable once wire protocol handles it
  override Bool supportsIdPrefixRename() { false }

  **
  ** Refs cross a process boundary — they are deserialized fresh each time,
  ** so identity checks must use equality rather than same-instance (verifySame).
  **
  override Void verifyIdsSame(Ref a, Ref b) { verifyEq(a, b) }

  **
  ** Dicts are deserialized fresh each time — use value equality, not identity.
  **
  override Void verifyRecSame(Dict? a, Dict? b)
  {
    if (a == null) { verify(b == null); return }
    test.verifyDictEq(a, b)
  }

  **
  ** Display string checks — delegate to standard behavior.
  ** Works because the Rust server includes dis in Ref serialization
  ** and the Fantom client sets Ref.disVal after deserialization.
  **
  override Void verifyDictDis(Dict r, Str expect)
  {
    verifyEq(r.dis, expect)
    verifyEq(r.id.dis, expect)
  }

  override Void verifyIdDis(Ref id, Str expect)
  {
    verifyEq(id.dis, expect)
  }
}
