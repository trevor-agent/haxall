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
** RustFolioProcess manages the lifecycle of the rust-folio subprocess.
**
** Ready signaling: Fantom's Process.out is an OutStream sink — it receives
** bytes written to the subprocess stdout on a background JVM thread, so
** there is no safe way to synchronously poll it from the folio actor thread
** without introducing locks or races. Instead, rust-folio writes its TCP
** port number to {dir}/.rust-folio.port after binding, and this class polls
** for that file. The file write is atomic from the OS perspective and safe
** to poll from any thread.
**
class RustFolioProcess
{
  ** Interval between port file polls.
  static const Duration pollInterval := 50ms

  ** Default timeout waiting for the port file (10 seconds).
  static const Duration readyTimeout := 10sec

  ** Default timeout waiting for the process to exit after Close.
  static const Duration exitTimeout := 5sec

  ** The binary name resolved at startup.
  private static const Str binaryName := "rust-folio"

  ** Port file name written by rust-folio after binding.
  static const Str portFileName := ".rust-folio.port"

  private Process? process

  ** Folio database directory.
  private File dir

  new make(File dir) { this.dir = dir }

  **
  ** Start the rust-folio process for the given folio directory.
  ** Returns the TCP port the process is listening on.
  **
  Int start(FolioConfig config)
  {
    // Clean up any stale port file from a previous run
    portFile.delete

    bin  := resolveBinary
    args := [bin.osPath, "--dir", dir.osPath]
    if (config.idPrefix != null)
      args.addAll(["--id-prefix", config.idPrefix])

    p := Process(args)
    p.mergeErr = true     // stderr → Env.out (logging)
    // Leave p.out as default (Env.cur.out) so Fantom receives the READY line
    // but we don't actually parse it — we use the port file instead.
    process = p
    p.run

    return waitForPortFile
  }

  **
  ** True if the process is believed to be running.
  ** (Fantom's Process has no isAlive query — we track state ourselves.)
  **
  Bool isRunning() { process != null }

  **
  ** OS-level liveness check.  rust-folio deletes its port file on clean
  ** shutdown; after a crash the file remains.  Combining the port file's
  ** presence with the spawned-process check gives a practical signal:
  **
  **   false — process was never started, or exited cleanly (port file gone)
  **   true  — process is running, or crashed without cleaning up the port file
  **
  ** Either way a true result means we must call kill() before respawning.
  **
  Bool isAlive() { process != null && portFile.exists }

  **
  ** Wait for the process to exit gracefully (after Close opcode was sent).
  ** Returns exit code or -1 on timeout.
  **
  Int waitForExit()
  {
    deadline := Duration.nowTicks + exitTimeout.ticks
    while (Duration.nowTicks < deadline)
    {
      try
      {
        // join() blocks; use a 100ms mini-timeout via Actor.sleep + kill check
        code := process?.join ?: 0
        process = null
        return code
      }
      catch (Err e)
      {
        // join() throws if process is still running (implementation dependent)
        Actor.sleep(100ms)
      }
    }
    return -1
  }

  **
  ** Forcibly kill the process. Called when Close + waitForExit times out.
  **
  Void kill()
  {
    try { process?.kill } catch (Err e) {}
    process = null
  }

//////////////////////////////////////////////////////////////////////////
// Internal
//////////////////////////////////////////////////////////////////////////

  ** The port file path: {dir}/.rust-folio.port
  File portFile() { dir + `$portFileName` }

  **
  ** Poll for the port file. rust-folio writes this file after binding the
  ** TCP listener. Returns the port number once the file appears.
  ** Throws TimeoutErr if the file does not appear within readyTimeout.
  **
  private Int waitForPortFile()
  {
    pf       := portFile
    deadline := Duration.nowTicks + readyTimeout.ticks

    while (Duration.nowTicks < deadline)
    {
      if (pf.exists)
      {
        content := pf.readAllStr.trim
        port    := content.toInt(10, false)
        if (port != null && port > 0 && port < 65536) return port
      }
      Actor.sleep(pollInterval)
    }

    // Timed out — kill the process
    kill
    throw TimeoutErr("rust-folio did not create port file within $readyTimeout (dir: ${dir.osPath})")
  }

  private File resolveBinary()
  {
    // 1. Alongside Fantom's bin/ dir (production install)
    binFile := Env.cur.homeDir + `bin/${binaryName}`
    if (binFile.exists) return binFile

    // 2. Dev environment: cargo release build adjacent to the haxall source
    devBin := Env.cur.homeDir +
      `../haxall/src/core/rustFolio/rust/target/release/${binaryName}`
    if (devBin.exists) return devBin

    // 3. Current directory (useful in tests)
    localBin := File.os(binaryName)
    if (localBin.exists) return localBin

    // 4. PATH fallback — let the OS resolve it
    return File.os(binaryName)
  }
}
