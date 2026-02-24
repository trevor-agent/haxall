// BenchFolio.fan — rustFolio integration benchmark harness
//
// Usage:
//   fan BenchFolio.fan [--backend rust|hx|both] [--recs N] [--iters N] [--warmup N]
//
// GC tracking (hxFolio only):
//   Redirect stderr to capture -verbose:gc output. Run the JVM with:
//     FAN_JAVA_OPTS="-verbose:gc" fan BenchFolio.fan --backend hx 2>gc.log
//   Then parse gc.log to extract pause count, total pause time, and max pause.
//   This explains p99 tail latency outliers observed in hxFolio results.
//
// Output:
//   Markdown table printed to stdout.

using xeto
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
        default:          Env.cur.err.printLine("Unknown arg: " + args[i])
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
    // Avoid nested string literals inside ${} — Fantom does not allow them
    Str dateStr := DateTime.now.toStr
    echo("> Date:   " + dateStr)
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
        }
        echo("")
      }
      finally { folio.close }
    }
    finally { try { deleteDir(dir) } catch {} }
  }

  ** Recursively delete a directory tree.
  private static Void deleteDir(File f)
  {
    if (f.isDir) f.list.each |c| { deleteDir(c) }
    f.delete
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
        "tz":    "UTC",
        "unit":  "kW",
      ])))
      if (batch.size >= 100) { flushBatch(folio, batch); batch.clear }
    }
    if (!batch.isEmpty) { flushBatch(folio, batch); batch.clear }

    // Collect point ids for history benchmarks
    folio.readAllList(Filter("his"), null).each |r| { hisIds.add(r.id) }

    // Seed 10k history items for first 10 points at 1-minute intervals.
    // Base: 2023-01-01 UTC; each point uses a non-overlapping timestamp range.
    Int limit    := 10.min(hisIds.size)
    Int hisCount := 10_000
    DateTime base := DateTime.make(2023, Month.jan, 1, 0, 0, 0, 0, TimeZone.utc)

    limit.times |pi|
    {
      HisItem[] items := HisItem[,]
      items.capacity = hisCount
      hisCount.times |ii|
      {
        ts  := base + 1min * (pi * hisCount + ii)
        val := Number.make((ii % 100).toFloat + 0.5f, Number.loadUnit("kW"))
        items.add(HisItem.make(ts, val))
      }
      folio.his().write(hisIds[pi], items).get(60sec)
    }

    echo("  Seeded: ${allIds.size} records, ${limit} points x ${hisCount} history items")
    echo("")
  }

  private Void flushBatch(Folio folio, Diff[] batch)
  {
    // commitAll(Diff[]) returns Diff[]; use Diff.id to get the assigned record id
    folio.commitAll(batch).each |d| { allIds.add(d.id) }
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

  private Void row(Str scenario, Str bname, Int[] durs)
  {
    sorted := durs.dup.sort
    Int sum := 0
    sorted.each |d| { sum += d }
    Float avgNs := sum.toFloat / sorted.size.toFloat
    Float opsSec := 1_000_000_000f / avgNs
    Float p50 := pct(sorted, 0.50f).toFloat / 1_000f
    Float p95 := pct(sorted, 0.95f).toFloat / 1_000f
    Float p99 := pct(sorted, 0.99f).toFloat / 1_000f

    // Extract to locals — Fantom does not allow nested string literals inside ${}
    Str opsStr := opsSec.toLocale("0").padl(10)
    Str p50Str := p50.toLocale("0.0").padl(8)
    Str p95Str := p95.toLocale("0.0").padl(8)
    Str p99Str := p99.toLocale("0.0").padl(8)
    Str sc := scenario.padr(37)
    Str nb := bname.padr(7)
    echo("| ${sc} | ${nb} | ${opsStr} | ${p50Str} | ${p95Str} | ${p99Str} |")
  }

  private static Int pct(Int[] sorted, Float p)
  {
    Int idx := (sorted.size.toFloat * p).toInt.min(sorted.size - 1)
    return sorted[idx]
  }

  //
  // --- Benchmark scenarios ---
  //

  private Void benchReadById(Folio folio, Str bname)
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
    row("readById (ipc floor)", bname, durs)
  }

  private Void benchReadAll(Folio folio, Str bname)
  {
    benchFilter(folio, bname, "readAll equip",           "equip")
    benchFilter(folio, bname, "readAll equip+point+his", "equip and point and his")
    benchFilter(folio, bname, "readAll no-match",        "ahu and chiller")
  }

  private Void benchFilter(Folio folio, Str bname, Str label, Str filterStr)
  {
    f := Filter(filterStr)
    durs := measure(iters) |->| { folio.readAllList(f, null) }
    row(label, bname, durs)
  }

  private Void benchReadCount(Folio folio, Str bname)
  {
    f := Filter("equip")
    durs := measure(iters * 2) |->| { folio.readCount(f, null) }
    row("readCount equip", bname, durs)
  }

  private Void benchCommit(Folio folio, Str bname)
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
        folio.commitAll(b)
      }
      Str label := "commitAll add batch=" + sz
      row(label, bname, durs)
    }
  }

  private Void benchHisWrite(Folio folio)
  {
    if (hisIds.isEmpty) return

    // Use 2020 as base to avoid overlap with seeded 2023 data
    DateTime base := DateTime.make(2020, Month.jan, 1, 0, 0, 0, 0, TimeZone.utc)
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
        folio.his().write(hisIds[0], items).get(30sec)
        callId[0] = callId[0] + 1
      }
      Str label := "hisWrite " + n + " items"
      row(label, "rust", durs)
    }
  }

  private Void benchHisRead(Folio folio)
  {
    if (hisIds.isEmpty) return

    // Span covers the 10k items seeded for hisIds[0] (base 2023-01-01, 1-min intervals)
    DateTime start := DateTime.make(2023, Month.jan, 1, 0, 0, 0, 0, TimeZone.utc)
    DateTime end   := start + 1min * 10_001  // just past the last seeded item
    Span span := Span.makeAbs(start, end)

    Int nIters := iters.min(200)
    durs := measure(nIters) |->|
    {
      HisItem[] result := HisItem[,]
      folio.his().read(hisIds[0], span, null) |item| { result.add(item) }
    }
    row("hisRead full (10k items)", "rust", durs)
  }
}
