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
               "inet @{fan.depend}",
               "xeto @{hx.depend}",
               "haystack @{hx.depend}",
               "folio @{hx.depend}"]
    srcDirs = [`fan/`]
    // Index registration enabled at M1+M2 combined milestone.
    // M2 provides functional filter evaluation, readAll, readCount — all
    // operations tested by testFolio are now implemented. Index was deferred
    // from M0/M1 to avoid UnsupportedErr propagation breaking flatfile/hx gates.
    index = ["testFolio.impl": "rustFolio::RustFolioTestImpl"]
  }
}
