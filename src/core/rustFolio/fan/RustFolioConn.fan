//
// Copyright (c) 2026, SkyFoundry LLC
// Licensed under the Academic Free License version 3.0
//
// History:
//   23 Feb 2026  Hathi  Creation (M1)
//

using concurrent
using inet
using xeto
using haystack
using folio

**
** RustFolioConn manages the TCP connection to the rust-folio process
** and sends/receives framed messages using the binary protocol.
**
** Design note: Unix domain sockets are not available in Fantom's Socket
** class. We use TCP on 127.0.0.1 with an ephemeral port assigned by the
** OS and communicated to us via the process's stdout ("READY:{port}").
** See PROGRESS.md DEV-003 for rationale.
**
** Thread safety: this class is NOT thread-safe. All calls must be made
** from the folio actor thread (single-threaded request processing).
**
class RustFolioConn
{
  // Message type bytes
  private static const Int reqType  := 0x01
  private static const Int respType := 0x02
  private static const Int errType  := 0x03

  // Protocol constants
  private static const Buf magic   := Buf().write('R').write('F').write('O').write('L').toImmutable
  private static const Int protoVer := 0x0001
  private static const Int maxMsgSize := 64 * 1024 * 1024

  // Opcode constants (must match Rust protocol.rs)
  static const Int opClose      := 0x0002
  static const Int opSync       := 0x0003
  static const Int opReadById   := 0x0010
  static const Int opReadByIds  := 0x0011
  static const Int opReadAll    := 0x0012
  static const Int opReadCount  := 0x0013
  static const Int opCommitAll  := 0x0020
  static const Int opCurVer     := 0x0030
  static const Int opFlushMode  := 0x0031
  static const Int opFlush      := 0x0032

  // Error wire codes (must match Rust error.rs)
  private static const Int errCodeUnknownRec        := 0x0001
  private static const Int errCodeCommit            := 0x0002
  private static const Int errCodeConcurrentChange  := 0x0003
  private static const Int errCodeShutdown          := 0x0004
  private static const Int errCodeDiff              := 0x0005
  private static const Int errCodeInvalidTagVal     := 0x0006

  private TcpSocket? socket

//////////////////////////////////////////////////////////////////////////
// Lifecycle
//////////////////////////////////////////////////////////////////////////

  ** Connect to the rust-folio process on the given port.
  Void connect(Int port)
  {
    s := TcpSocket()
    s.connect(IpAddr("127.0.0.1"), port)
    socket = s
    doHandshake(s)
  }

  ** Send Close opcode and disconnect.
  Void close()
  {
    if (socket == null) return
    try
    {
      sendClose
    }
    catch (Err e) {}
    try { socket.close } catch (Err e) {}
    socket = null
  }

  private Void doHandshake(TcpSocket s)
  {
    out := s.out
    in  := s.in

    // Send: 4 bytes magic + 2 bytes version
    out.writeBuf(magic.seek(0))
    out.writeI2(protoVer)
    out.flush

    // Read: 4 bytes magic + 2 bytes version + 1 byte status
    // readBufFully on InStream fills the Buf from the stream
    respBuf := in.readBufFully(null, 7).seek(0)
    // Compare magic bytes 'R','F','O','L'
    if (respBuf.read != 'R' || respBuf.read != 'F' ||
        respBuf.read != 'O' || respBuf.read != 'L')
      throw IOErr("Bad magic bytes in server handshake response")
    respBuf.readU2  // server version (ignored for now)
    status := respBuf.read
    if (status != 0x00)
      throw IOErr("Server rejected handshake (status=$status) — protocol version mismatch")
  }

//////////////////////////////////////////////////////////////////////////
// Requests
//////////////////////////////////////////////////////////////////////////

  ** Close opcode — server acknowledges then shuts down.
  Void sendClose()
  {
    sendRequest(opClose, Buf())
    readResponse(opClose) // drain the ack
  }

  ** Sync — server flushes (single-threaded, always immediate).
  Void sendSync()
  {
    sendRequest(opSync, Buf())
    readResponse(opSync)
  }

  ** Read current persistent version.
  Int readCurVer()
  {
    sendRequest(opCurVer, Buf())
    resp := readResponse(opCurVer)
    return resp.readS8
  }

  ** Read flush mode (get=0x00) or set (set=0x01 + mode string).
  Str readFlushMode()
  {
    payload := Buf()
    payload.write(0x00) // get
    sendRequest(opFlushMode, payload)
    resp := readResponse(opFlushMode)
    return RustFolioSerializer.readStr(resp.in)
  }

  Void setFlushMode(Str mode)
  {
    payload := Buf()
    payload.write(0x01) // set
    RustFolioSerializer.writeStr(payload.out, mode)
    sendRequest(opFlushMode, payload)
    readResponse(opFlushMode)
  }

  ** Flush (redb fsync is automatic on commit, so this is a no-op).
  Void sendFlush()
  {
    sendRequest(opFlush, Buf())
    readResponse(opFlush)
  }

  ** ReadById — returns the Dict or null if not found.
  Dict? readById(Ref id)
  {
    payload := Buf()
    RustFolioSerializer.writeRef(payload.out, id)
    sendRequest(opReadById, payload)
    resp := readResponse(opReadById)
    return decodeOptRec(resp.in)
  }

  ** ReadByIds — returns Dict? list (null for each not-found id).
  Dict?[] readByIds(Ref[] ids)
  {
    payload := Buf()
    payload.out.writeI4(ids.size)
    ids.each |id| { RustFolioSerializer.writeRef(payload.out, id) }
    sendRequest(opReadByIds, payload)
    resp   := readResponse(opReadByIds)
    in     := resp.in
    count  := in.readU4
    result := Dict?[,]
    count.times { result.add(decodeOptRec(in)) }
    return result
  }

  ** ReadAll — returns all matching Dicts as a list.
  Dict[] readAll(Filter filter, Dict? opts)
  {
    payload := Buf()
    RustFolioSerializer.writeStr(payload.out, filter.toStr)
    RustFolioSerializer.writeDict(payload.out, opts ?: Etc.emptyDict)
    sendRequest(opReadAll, payload)
    resp  := readResponse(opReadAll)
    in    := resp.in
    count := in.readU4
    result := Dict[,]
    count.times { result.add(RustFolioSerializer.readDict(in)) }
    return result
  }

  ** ReadCount — returns count of matching records.
  Int readCount(Filter filter, Dict? opts)
  {
    payload := Buf()
    RustFolioSerializer.writeStr(payload.out, filter.toStr)
    RustFolioSerializer.writeDict(payload.out, opts ?: Etc.emptyDict)
    sendRequest(opReadCount, payload)
    resp := readResponse(opReadCount)
    return resp.readS8
  }

  ** CommitAll — apply a batch of Diffs. Returns CommitResult list.
  RustCommitResult[] commitAll(Diff[] diffs)
  {
    payload := Buf()
    payload.out.writeI4(diffs.size)
    diffs.each |d| { RustFolioSerializer.writeDiff(payload.out, d) }
    sendRequest(opCommitAll, payload)
    resp  := readResponse(opCommitAll)
    in    := resp.in
    count := in.readU4
    results := RustCommitResult[,]
    count.times
    {
      id     := RustFolioSerializer.readRef(in)
      oldMod := RustFolioSerializer.readOptDateTime(in)
      newMod := RustFolioSerializer.readOptDateTime(in)
      oldRec := RustFolioSerializer.readOptDict(in)
      newRec := RustFolioSerializer.readOptDict(in)
      results.add(RustCommitResult { it.id = id; it.oldMod = oldMod; it.newMod = newMod; it.oldRec = oldRec; it.newRec = newRec })
    }
    return results
  }

//////////////////////////////////////////////////////////////////////////
// Framing
//////////////////////////////////////////////////////////////////////////

  private Void sendRequest(Int op, Buf payload)
  {
    if (socket == null) throw ShutdownErr("RustFolio connection is closed")
    out := socket.out

    // total_len = 1 (type) + 2 (opcode) + payload.size
    totalLen := 1 + 2 + payload.size

    out.writeI4(totalLen)
    out.write(reqType)
    out.writeI2(op)
    if (payload.size > 0) out.writeBuf(payload.seek(0))
    out.flush
  }

  private Buf readResponse(Int expectedOp)
  {
    if (socket == null) throw ShutdownErr("RustFolio connection is closed")
    in := socket.in

    // Read 4-byte total length
    totalLen := in.readU4
    if (totalLen > maxMsgSize)
      throw IOErr("Response too large: $totalLen bytes")

    // Read body
    bodyBuf := Buf(totalLen)
    in.readBufFully(bodyBuf, totalLen)
    bodyBuf.seek(0)

    mtype := bodyBuf.read
    op    := bodyBuf.readU2

    if (op != expectedOp)
      throw IOErr("Opcode mismatch: expected 0x${expectedOp.toHex(4)}, got 0x${op.toHex(4)}")

    if (mtype == errType)
    {
      errCode := bodyBuf.readU2
      errMsg  := RustFolioSerializer.readStr(bodyBuf.in)
      throwError(errCode, errMsg)
    }

    if (mtype != respType)
      throw IOErr("Unexpected message type: 0x${mtype.toHex(2)}")

    return bodyBuf
  }

  private Dict? decodeOptRec(InStream in)
  {
    flag := in.read
    if (flag == 0x00) return null
    return RustFolioSerializer.readDict(in)
  }

  ** Map wire error codes to Fantom exceptions.
  private static Void throwError(Int code, Str msg)
  {
    switch (code)
    {
      case errCodeUnknownRec:       throw UnknownRecErr(msg)
      case errCodeCommit:           throw CommitErr(msg)
      case errCodeConcurrentChange: throw ConcurrentChangeErr(msg)
      case errCodeShutdown:         throw ShutdownErr(msg)
      case errCodeDiff:             throw DiffErr(msg)
      default:                      throw IOErr("rust-folio error [$code.toHex(4)]: $msg")
    }
  }

  private static Buf readBytesExact(InStream in, Int n, Str ctx)
  {
    buf := Buf(n)
    in.readBufFully(buf, n)
    return buf.seek(0)
  }

}

**************************************************************************
** RustCommitResult
**************************************************************************

** Result of a single diff from a CommitAll call.
const class RustCommitResult
{
  new make(|This| f) { f(this) }
  const Ref       id
  const DateTime? oldMod
  const DateTime? newMod
  const Dict?     oldRec
  const Dict?     newRec
}
