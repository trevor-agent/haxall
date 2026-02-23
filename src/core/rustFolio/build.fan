#! /usr/bin/env fan
//
// Copyright (c) 2026, SkyFoundry LLC
// Licensed under the Academic Free License version 3.0
//
// History:
//   23 Feb 2026  Hathi  Creation (M0 scaffold)
//

using build

**
** Build: rustFolio
**
class Build : BuildPod
{
  new make()
  {
    podName = "rustFolio"
    summary = "Rust-backed Folio implementation"
    meta    = ["org.name":     "SkyFoundry",
               "org.uri":      "https://skyfoundry.com/",
               "proj.name":    "Haxall",
               "proj.uri":     "https://haxall.io/",
               "license.name": "Academic Free License 3.0",
               "vcs.name":     "Git",
               "vcs.uri":      "https://github.com/haxall/haxall"
              ]
    depends = ["sys @{fan.depend}",
               "concurrent @{fan.depend}",
               "util @{fan.depend}",
               "xeto @{hx.depend}",
               "haystack @{hx.depend}",
               "folio @{hx.depend}"]
    srcDirs = [`fan/`]
    // NOTE: index registration ("testFolio.impl") deferred to M2.
    // Rationale: AbstractFolioTest.runImpls iterates all impls via each{} which
    // does not catch per-impl exceptions. When rustfolio stubs throw UnsupportedErr,
    // the exception propagates and prevents flatfile/hx from running in that test
    // method. Enable once M2 provides functional commits+reads.
    // index = ["testFolio.impl": "rustFolio::RustFolioTestImpl"]
  }
}
