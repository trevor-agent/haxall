**
** BenchFolio.fan — rustFolio integration benchmark harness
**
** Usage:
**   fan BenchFolio.fan [--backend rust|hx|both] [--recs N] [--iters N] [--warmup N]
**
** GC tracking (hxFolio only):
**   Redirect stderr to capture -verbose:gc output. Run the JVM with:
**     FAN_JAVA_OPTS="-verbose:gc" fan BenchFolio.fan --backend hx 2>gc.log
**   Then parse gc.log to extract pause count, total pause time, and max pause.
**   This explains p99 tail latency outliers observed in hxFolio results.
**
** Output:
**   Markdown table printed to stdout.
**

using folio
using hxFolio
using rustFolio
using haystack
using concurrent

class BenchFolio
{
  //
  // --- Options ---
  //
  Str backend := "both"   // rust | hx | both
  Int recs    := 10_000
  Int iters   := 1_000
  Int warmup  := 200

  //
  // --- Per-backend seeded-data state ---
  //
  Ref[] allIds := Ref[,]  // all committed record ids
  Ref[] hisIds := Ref[,]  // ids of records with 'his' tag

  //
  // --- Entry point ---
  //
  static Void main(Str[] args)
  {
    b := BenchFolio()
    b.parseArgs(args)
    b.run
  }

  private Void parseArgs(Str[] args)
  {
    Int i := 0
    while (i < args.size)
    {
      switch (args[i])
      {
        case "--backend": backend = args[++i]
        case "--recs":    recs    = args[++i].toInt
        case "--iters":   iters   = args[++i].toInt
        case "--warmup":  warmup  = args[++i].toInt
        default:          Env.cur.err.printLine("Unknown arg: ${args[i]}")
      }
      i++
    }
  }

  //
  // --- Top-level runner ---
  //
  Void run()
  {
    echo("# rustFolio Benchmark Results")
    echo("")
    echo("> Date:   ${DateTime.now.toLocale("DD-Mon-YYYY hh:mm:ss")}")
    echo("> Recs:   ${recs}  |  Iters: ${iters}  |  Warmup: ${warmup}")
    echo("")
    if (backend == "rust" || backend == "both") runBackend("rust")
    if (backend == "hx"   || backend == "both") runBackend("hx")
  }

  private Void runBackend(Str name)
  {
    allIds.clear
    hisIds.clear

    dir := Env.cur.workDir + `bench_tmp_${name}_${Duration.now.ticks}/`
    dir.create
    try
    {
      config := FolioConfig { it.dir = dir; it.log = Log.get("bench") }
      Folio folio := name == "rust" ? RustFolio.open(config) : HxFolio.open(config)
      try
      {
        echo("## Backend: ${name}")
        echo("")
        seed(folio)

        printHeader
        benchReadById(folio, name)
        benchReadAll(folio, name)
        benchReadCount(folio, name)
        benchCommit(folio, name)
        if (name == "rust")
        {
          benchHisWrite(folio)
          benchHisRead(folio)
          benchHisStat(folio)
        }
        echo("")
      }
      finally { folio.close }
    }
    finally { try { dir.delete(true) } catch {} }
  }

  //
  // --- Seeding ---
  //
  private Void seed(Folio folio)
  {
    nSites  := (recs / 10).max(1)
    nEquips := (recs / 4).max(1)
    nPoints := (recs - nSites - nEquips).max(1)

    Diff[] batch := Diff[,]

    nSites.times |i|
    {
      city := i % 2 == 0 ? "Chicago" : "NewYork"
      batch.add(Diff.makeAdd(Etc.makeDict([
        "dis":     "Site-${i}",
        "geoCity": city,
        "site":    Marker.val,
      ])))
      if (batch.size >= 100) { flushBatch(folio, batch); batch.clear }
    }

    nEquips.times |i|
    {
      batch.add(Diff.makeAdd(Etc.makeDict([
        "ahu":   Marker.val,
        "dis":   "Equip-${i}",
        "equip": Marker.val,
      ])))
      if (batch.size >= 100) { flushBatch(folio, batch); batch.clear }
    }

    nPoints.times |i|
    {
      batch.add(Diff.makeAdd(Etc.makeDict([
        "dis":   "Point-${i}",
        "equip": Marker.val,
        "his":   Marker.val,
        "kind":  "Number",
        "point": Marker.val,
        "unit":  Number.loadUnit("kW"),
      ])))
      if (batch.size >= 100) { flushBatch(folio, batch); batch.clear }
    }
    if (!batch.isEmpty) { flushBatch(folio, batch); batch.clear }

    // Collect point ids for history benchmarks
    folio.readAll("his", null).each |r| { hisIds.add(r.id) }

    // Seed 10k history items for first 10 points at 1-minute intervals
    // Base: 2023-01-01 UTC; each point uses a non-overlapping range
    Int limit    := 10.min(hisIds.size)
    Int hisCount := 10_000
    DateTime base := DateTime.make(2023, Month.jan, 1, 0, 0, 0, TimeZone.utc)

    limit.times |pi|
    {
      pt := folio.readById(hisIds[pi])
      HisItem[] items := HisItem[,]
      items.capacity = hisCount
      hisCount.times |ii|
      {
        ts  := base + 1min * (pi * hisCount + ii)
        val := Number.make((ii % 100).toFloat + 0.5f, Number.loadUnit("kW"))
        items.add(HisItem.make(ts, val))
      }
      folio.hisWrite(pt, items, null, null).get(60sec)
    }

    echo("  Seeded: ${allIds.size} records, ${limit} points x ${hisCount} history items")
    echo("")
  }

  private Void flushBatch(Folio folio, Diff[] batch)
  {
    folio.commitAll(batch, null).each |r| { allIds.add(r.id) }
  }

  //
  // --- Timing infrastructure ---
  //

  **
  ** Run 'n' timed iterations preceded by warmup iterations (untimed).
  ** Warmup count is capped at min(warmup, n/2) so it never exceeds
  ** the measurement set — critical for low-iteration scenarios.
  ** Returns Int[] of per-iteration elapsed nanoseconds.
  **
  private Int[] measure(Int n, |->| op)
  {
    Int w := warmup.min(n / 2).max(1)
    w.times { op() }
    Int[] durs := Int[,]
    durs.capacity = n
    n.times
    {
      Int t1 := Duration.now.ticks
      op()
      durs.add(Duration.now.ticks - t1)
    }
    return durs
  }

  private Void printHeader()
  {
    echo("| Scenario                              | Backend | ops/sec    | p50 µs   | p95 µs   | p99 µs   |")
    echo("|---------------------------------------|---------|------------|----------|----------|----------|")
  }

  private Void row(Str scenario, Str name, Int[] durs)
  {
    sorted := durs.dup.sort
    Int sum := 0
    sorted.each |d| { sum += d }
    Float avgNs := sum.toFloat / sorted.size.toFloat
    Float opsSec := 1_000_000_000f / avgNs
    Float p50 := pct(sorted, 0.50f).toFloat / 1_000f
    Float p95 := pct(sorted, 0.95f).toFloat / 1_000f
    Float p99 := pct(sorted, 0.99f).toFloat / 1_000f
    Str sc := scenario.padr(37)
    Str nb := name.padr(7)
    echo("| ${sc} | ${nb} | ${opsSec.toLocale("0").padl(10)} | ${p50.toLocale("0.0").padl(8)} | ${p95.toLocale("0.0").padl(8)} | ${p99.toLocale("0.0").padl(8)} |")
  }

  private static Int pct(Int[] sorted, Float p)
  {
    Int idx := (sorted.size.toFloat * p).toInt.min(sorted.size - 1)
    return sorted[idx]
  }

  //
  // --- Benchmark scenarios ---
  //

  private Void benchReadById(Folio folio, Str name)
  {
    if (allIds.isEmpty) return
    // High iteration count — this is the IPC floor measurement.
    // The delta between rust and hx backends is pure IPC overhead per call.
    Int n := iters * 5
    Int[] ci := [0]
    durs := measure(n) |->|
    {
      folio.readById(allIds[ci[0] % allIds.size], false)
      ci[0] = ci[0] + 1
    }
    row("readById (ipc floor)", name, durs)
  }

  private Void benchReadAll(Folio folio, Str name)
  {
    [
      ["readAll equip",           "equip"],
      ["readAll equip+point+his", "equip and point and his"],
      ["readAll no-match",        "ahu and chiller"],
    ].each |pair|
    {
      Str label  := pair[0]
      Str filter := pair[1]
      durs := measure(iters) |->| { folio.readAll(filter, null) }
      row(label, name, durs)
    }
  }

  private Void benchReadCount(Folio folio, Str name)
  {
    durs := measure(iters * 2) |->| { folio.readCount("equip", null) }
    row("readCount equip", name, durs)
  }

  private Void benchCommit(Folio folio, Str name)
  {
    [1, 10, 100].each |sz|
    {
      Int n := (iters / sz).max(50)
      Int[] ci := [0]
      durs := measure(n) |->|
      {
        Diff[] b := Diff[,]
        b.capacity = sz
        sz.times
        {
          b.add(Diff.makeAdd(Etc.makeDict([
            "bench": Marker.val,
            "dis":   "B-${ci[0]}",
          ])))
          ci[0] = ci[0] + 1
        }
        folio.commitAll(b, null)
      }
      row("commitAll add batch=${sz}", name, durs)
    }
  }

  private Void benchHisWrite(Folio folio)
  {
    if (hisIds.isEmpty) return
    Dict pt := folio.readById(hisIds[0])

    // Use 2020 as base to avoid overlap with seeded 2023 data
    DateTime base := DateTime.make(2020, Month.jan, 1, 0, 0, 0, TimeZone.utc)
    Int[] callId  := [0]

    [100, 1_000, 10_000].each |n|
    {
      Int nIters := (500 / n).max(5)
      durs := measure(nIters) |->|
      {
        HisItem[] items := HisItem[,]
        items.capacity = n
        n.times |i|
        {
          ts  := base + 1min * (callId[0] * n + i)
          val := Number.make((i % 100).toFloat, Number.loadUnit("kW"))
          items.add(HisItem.make(ts, val))
        }
        folio.hisWrite(pt, items, null, null).get(30sec)
        callId[0] = callId[0] + 1
      }
      row("hisWrite ${n} items", "rust", durs)
    }
  }

  private Void benchHisRead(Folio folio)
  {
    if (hisIds.isEmpty) return
    Dict pt := folio.readById(hisIds[0])

    // Span covers the 10k items seeded for hisIds[0] (base 2023-01-01, 1-min intervals)
    DateTime start := DateTime.make(2023, Month.jan, 1, 0, 0, 0, TimeZone.utc)
    DateTime end   := start + 1min * 10_001  // just past the last seeded item

    Int nIters := iters.min(200)
    durs := measure(nIters) |->| { folio.hisRead(pt, Span.make(start, end), null) }
    row("hisRead full (10k items)", "rust", durs)
  }

  private Void benchHisStat(Folio folio)
  {
    if (hisIds.isEmpty) return
    Dict pt := folio.readById(hisIds[0])
    durs := measure(iters * 5) |->| { folio.hisStat(pt, null) }
    row("hisStat", "rust", durs)
  }
}
