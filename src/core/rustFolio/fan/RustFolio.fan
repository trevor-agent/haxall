//
// Copyright (c) 2026, SkyFoundry LLC
// Licensed under the Academic Free License version 3.0
//
// History:
//   23 Feb 2026  Hathi  Creation (M0 scaffold)
//

using concurrent
using xeto
using haystack
using folio

**
** RustFolio: Rust-backed Folio implementation.
**
** The Rust process handles record persistence, filter evaluation,
** history storage, and display string computation. It communicates
** with the Fantom/Haxall ecosystem via a binary protocol over
** Unix domain sockets.
**
** Milestone status: M0 scaffold — all storage methods stubbed.
**
const class RustFolio : Folio
{

//////////////////////////////////////////////////////////////////////////
// Construction
//////////////////////////////////////////////////////////////////////////

  **
  ** Open database for given configuration.
  **
  static Folio open(FolioConfig config)
  {
    return make(config)
  }

  ** Constructor for open
  private new make(FolioConfig config) : super(config)
  {
    this.passwords = PasswordStore.open(dir+`passwords.props`, config)
  }

//////////////////////////////////////////////////////////////////////////
// Identity
//////////////////////////////////////////////////////////////////////////

  ** Password storage (managed Fantom-side, no Rust involvement)
  const override PasswordStore passwords

//////////////////////////////////////////////////////////////////////////
// Storage Metadata
//////////////////////////////////////////////////////////////////////////

  ** Current persistent version — implemented in M1
  override Int curVer()
  {
    throw UnsupportedErr("RustFolio.curVer: not implemented until M1")
  }

  ** Flush mode — implemented in M1
  override Str flushMode
  {
    get { throw UnsupportedErr("RustFolio.flushMode.get: not implemented until M1") }
    set { throw UnsupportedErr("RustFolio.flushMode.set: not implemented until M1") }
  }

  ** Flush dirty data — implemented in M1
  override Void flush()
  {
    throw UnsupportedErr("RustFolio.flush: not implemented until M1")
  }

//////////////////////////////////////////////////////////////////////////
// Subsystems
//////////////////////////////////////////////////////////////////////////

  ** Backup — not supported in v1
  override FolioBackup backup()
  {
    throw UnsupportedErr("RustFolio.backup: not supported")
  }

  ** History — implemented in M5
  override FolioHis his()
  {
    throw UnsupportedErr("RustFolio.his: not implemented until M5")
  }

  ** File storage — not supported in v1
  override FolioFile file()
  {
    throw UnsupportedErr("RustFolio.file: not supported")
  }

//////////////////////////////////////////////////////////////////////////
// Lifecycle
//////////////////////////////////////////////////////////////////////////

  **
  ** Close the database asynchronously.
  ** Returns a valid FolioFuture so teardown works cleanly.
  ** In M1 this will send a Close opcode to the Rust process and wait.
  **
  override protected FolioFuture doCloseAsync()
  {
    return FolioFuture.makeSync(CountFolioRes(0))
  }

//////////////////////////////////////////////////////////////////////////
// Reads (stubbed, implemented in M1/M2)
//////////////////////////////////////////////////////////////////////////

  override protected FolioRec? doReadRecById(Ref id)
  {
    throw UnsupportedErr("RustFolio.doReadRecById: not implemented until M1")
  }

  override protected FolioFuture doReadByIds(Ref[] ids)
  {
    throw UnsupportedErr("RustFolio.doReadByIds: not implemented until M1")
  }

  override protected FolioFuture doReadAll(Filter filter, Dict? opts)
  {
    throw UnsupportedErr("RustFolio.doReadAll: not implemented until M2")
  }

  override protected Int doReadCount(Filter filter, Dict? opts)
  {
    throw UnsupportedErr("RustFolio.doReadCount: not implemented until M2")
  }

  override protected Obj? doReadAllEachWhile(Filter filter, Dict? opts, |Dict->Obj?| f)
  {
    throw UnsupportedErr("RustFolio.doReadAllEachWhile: not implemented until M2")
  }

//////////////////////////////////////////////////////////////////////////
// Commits (stubbed, implemented in M1)
//////////////////////////////////////////////////////////////////////////

  override protected FolioFuture doCommitAllAsync(Diff[] diffs, Obj? cxInfo)
  {
    throw UnsupportedErr("RustFolio.doCommitAllAsync: not implemented until M1")
  }

}
