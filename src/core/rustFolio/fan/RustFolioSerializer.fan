//
// Copyright (c) 2026, SkyFoundry LLC
// Licensed under the Academic Free License version 3.0
//
// History:
//   23 Feb 2026  Hathi  Creation (M1)
//

using xeto
using haystack
using folio

**
** RustFolioSerializer converts between Fantom/Haystack types and the
** rust-folio binary wire protocol. All methods operate on InStream/OutStream.
**
** Type tag bytes (must match Rust types_ser.rs):
**   0x00 Null, 0x01 Marker, 0x02 NA, 0x03 Remove/None
**   0x04 Bool, 0x05 Number, 0x06 Str, 0x07 Ref, 0x08 Uri
**   0x09 Date, 0x0A Time, 0x0B DateTime, 0x0C Coord
**   0x0D XStr, 0x0E Symbol, 0x0F Span
**   0x10 List, 0x11 Dict, 0x12 Grid
**
const class RustFolioSerializer
{

//////////////////////////////////////////////////////////////////////////
// Type Tags
//////////////////////////////////////////////////////////////////////////

  static const Int tagNull     := 0x00
  static const Int tagMarker   := 0x01
  static const Int tagNA       := 0x02
  static const Int tagRemove   := 0x03
  static const Int tagBool     := 0x04
  static const Int tagNumber   := 0x05
  static const Int tagStr      := 0x06
  static const Int tagRef      := 0x07
  static const Int tagUri      := 0x08
  static const Int tagDate     := 0x09
  static const Int tagTime     := 0x0A
  static const Int tagDateTime := 0x0B
  static const Int tagCoord    := 0x0C
  static const Int tagXStr     := 0x0D
  static const Int tagSymbol   := 0x0E
  static const Int tagSpan     := 0x0F
  static const Int tagList     := 0x10
  static const Int tagDict     := 0x11
  static const Int tagGrid     := 0x12

//////////////////////////////////////////////////////////////////////////
// Write (Fantom → wire)
//////////////////////////////////////////////////////////////////////////

  ** Write a Haystack value to the output stream.
  static Void writeVal(OutStream out, Obj? val)
  {
    if (val == null)          { out.write(tagNull);   return }
    if (val === Marker.val)   { out.write(tagMarker); return }
    if (val === NA.val)       { out.write(tagNA);     return }
    if (val === None.val)     { out.write(tagRemove); return }

    if (val is Bool)
    {
      out.write(tagBool)
      out.write(val ? 1 : 0)
      return
    }

    if (val is Number)
    {
      n := (Number)val
      out.write(tagNumber)
      out.writeF8(n.toFloat)
      writeStr(out, n.unit?.symbol ?: "")
      return
    }

    if (val is Str)
    {
      out.write(tagStr)
      writeStr(out, val)
      return
    }

    if (val is Ref)
    {
      out.write(tagRef)
      writeRef(out, val)
      return
    }

    if (val is Uri)
    {
      out.write(tagUri)
      writeStr(out, val.toStr)
      return
    }

    if (val is Date)
    {
      d := (Date)val
      out.write(tagDate)
      out.writeI2(d.year)
      out.write(d.month.ordinal + 1)
      out.write(d.day)
      return
    }

    if (val is Time)
    {
      t := (Time)val
      out.write(tagTime)
      out.write(t.hour)
      out.write(t.min)
      out.write(t.sec)
      out.writeI4(t.nanoSec)
      return
    }

    if (val is DateTime)
    {
      out.write(tagDateTime)
      writeDateTime(out, val)
      return
    }

    if (val is Coord)
    {
      c := (Coord)val
      out.write(tagCoord)
      out.writeF8(c.lat)
      out.writeF8(c.lng)
      return
    }

    if (val is XStr)
    {
      x := (XStr)val
      out.write(tagXStr)
      writeStr(out, x.type)
      writeStr(out, x.val)
      return
    }

    if (val is Symbol)
    {
      out.write(tagSymbol)
      writeStr(out, val.toStr)
      return
    }

    if (val is Span)
    {
      out.write(tagSpan)
      writeStr(out, val.toStr)
      return
    }

    if (val is List)
    {
      items := (List)val
      out.write(tagList)
      out.writeI4(items.size)
      items.each |item| { writeVal(out, item) }
      return
    }

    if (val is Dict)
    {
      out.write(tagDict)
      writeDict(out, val)
      return
    }

    if (val is Grid)
    {
      out.write(tagGrid)
      writeGrid(out, val)
      return
    }

    // Unknown type — serialize as its string representation
    out.write(tagStr)
    writeStr(out, val.toStr)
  }

  ** Write a Haystack Ref: id string + dis string (empty if null).
  static Void writeRef(OutStream out, Ref r)
  {
    writeStr(out, r.id)
    writeStr(out, r.disVal ?: "")
  }

  ** Write a Dict: 4-byte tag count + name/value pairs.
  static Void writeDict(OutStream out, Dict d)
  {
    // Count tags (Dict has no size method — must iterate)
    n := 0
    d.each |v, name| { n++ }
    out.writeI4(n)
    d.each |v, name|
    {
      writeStr(out, name)
      writeVal(out, v)
    }
  }

  ** Write an optional Dict: 0x00 = null, 0x01 + dict = present.
  static Void writeOptDict(OutStream out, Dict? d)
  {
    if (d == null) { out.write(0x00); return }
    out.write(0x01)
    writeDict(out, d)
  }

  ** Write a DateTime: year(2) month(1) day(1) hour(1) min(1) sec(1) nanos(4) tzName.
  ** Uses dt.tz.fullName (e.g. "America/Los_Angeles") so the Rust side can parse
  ** it with chrono-tz, which requires full IANA names.
  static Void writeDateTime(OutStream out, DateTime dt)
  {
    out.writeI2(dt.year)
    out.write(dt.month.ordinal + 1)
    out.write(dt.day)
    out.write(dt.hour)
    out.write(dt.min)
    out.write(dt.sec)
    out.writeI4(dt.nanoSec)
    writeStr(out, dt.tz.fullName)
  }

  ** Write an optional DateTime: 0x00 = null, 0x01 + datetime = present.
  static Void writeOptDateTime(OutStream out, DateTime? dt)
  {
    if (dt == null) { out.write(0x00); return }
    out.write(0x01)
    writeDateTime(out, dt)
  }

  ** Write a length-prefixed UTF-8 string.
  ** Short strings (< 0xFFFF bytes): 2-byte length prefix.
  ** Long strings: 0xFFFF marker + 4-byte length prefix.
  static Void writeStr(OutStream out, Str s)
  {
    utf8 := s.toBuf(Charset.utf8)
    len  := utf8.size
    if (len < 0xFFFF)
    {
      out.writeI2(len)
    }
    else
    {
      out.writeI2(0xFFFF)
      out.writeI4(len)
    }
    out.writeBuf(utf8.seek(0))
  }

  ** Write a Diff to the wire.
  static Void writeDiff(OutStream out, Diff d)
  {
    out.write(diffFlags(d))
    writeRef(out, d.id)
    writeOptDateTime(out, d.oldMod)
    writeDict(out, d.changes)
  }

  private static Int diffFlags(Diff d)
  {
    flags := 0
    if (d.isAdd)       flags = flags.or(0x01)
    if (d.isRemove)    flags = flags.or(0x02)
    if (d.isTransient) flags = flags.or(0x04)
    if (d.isForce)     flags = flags.or(0x08)
    return flags
  }

  private static Void writeGrid(OutStream out, Grid g)
  {
    writeDict(out, g.meta)
    out.writeI4(g.cols.size)
    g.cols.each |c| { writeStr(out, c.name) }
    out.writeI4(g.size)
    g.each |r| { writeDict(out, r) }
  }

//////////////////////////////////////////////////////////////////////////
// Read (wire → Fantom)
//////////////////////////////////////////////////////////////////////////

  ** Read a Haystack value from the input stream.
  static Obj? readVal(InStream in)
  {
    tag := in.read
    switch (tag)
    {
      case tagNull:     return null
      case tagMarker:   return Marker.val
      case tagNA:       return NA.val
      case tagRemove:   return None.val
      case tagBool:     return in.read != 0
      case tagNumber:
        f    := in.readF8
        unit := readStr(in)
        return unit.isEmpty ? Number(f) : Number(f, Unit(unit))
      case tagStr:      return readStr(in)
      case tagRef:      return readRef(in)
      case tagUri:      return `${readStr(in)}`
      case tagDate:     return readDate(in)
      case tagTime:     return readTime(in)
      case tagDateTime: return readDateTime(in)
      case tagCoord:
        lat := in.readF8
        lng := in.readF8
        return Coord(lat, lng)
      case tagXStr:
        ty  := readStr(in)
        val := readStr(in)
        return XStr(ty, val)
      case tagSymbol:   return Symbol.fromStr(readStr(in))
      case tagSpan:     return Span.fromStr(readStr(in))
      case tagList:
        count := in.readU4
        items := Obj?[,]
        count.times { items.add(readVal(in)) }
        // Coerce to a typed list when all elements share the same non-null type.
        // Fantom verifyListEq checks typeof, so Span[] != Obj?[]. We rebuild the
        // list with the detected element type so round-trip types match.
        if (!items.isEmpty)
        {
          elemType := items.first?.typeof
          if (elemType != null && items.all |x| { x?.typeof == elemType })
          {
            typed := elemType.emptyList.rw
            items.each |x| { typed.add(x) }
            return typed
          }
        }
        return items
      case tagDict:     return readDict(in)
      case tagGrid:     return readGrid(in)
      default: throw IOErr("Unknown type tag: 0x${tag.toHex(2)}")
    }
  }

  ** Read an HRef from the stream.
  static Ref readRef(InStream in)
  {
    id  := readStr(in)
    dis := readStr(in)
    return dis.isEmpty ? Ref(id) : Ref(id, dis)
  }

  ** Read a Dict from the stream.
  static Dict readDict(InStream in)
  {
    count := in.readU4
    map   := Str:Obj?[:]
    count.times
    {
      name := readStr(in)
      val  := readVal(in)
      // null means Null type tag (absent tag) — skip it
      // None.val (Remove) is non-null and must be preserved (for changes dicts)
      if (val != null) map[name] = val
    }
    return Etc.makeDict(map)
  }

  ** Read an optional Dict from the stream.
  static Dict? readOptDict(InStream in)
  {
    flag := in.read
    return flag == 0x00 ? null : readDict(in)
  }

  ** Read a DateTime from the stream.
  static DateTime readDateTime(InStream in)
  {
    year   := in.readU2
    month  := Month.vals[in.read - 1]
    day    := in.read
    hour   := in.read
    min    := in.read
    sec    := in.read
    nanos  := in.readU4
    tzName := readStr(in)
    tz     := TimeZone.fromStr(tzName)
    return DateTime.make(year, month, day, hour, min, sec, nanos, tz)
  }

  ** Read an optional DateTime from the stream.
  static DateTime? readOptDateTime(InStream in)
  {
    flag := in.read
    return flag == 0x00 ? null : readDateTime(in)
  }

  ** Read a length-prefixed UTF-8 string.
  ** Note: readBufFully writes to buf starting at pos=0 but leaves pos=0
  ** (Fantom heap buf semantics). Do NOT call flip() — it would set size=pos=0.
  ** buf.size is already set to len; just read from pos 0 directly.
  static Str readStr(InStream in)
  {
    len := in.readU2
    if (len == 0xFFFF) len = in.readU4
    if (len == 0) return ""
    buf := Buf(len)
    in.readBufFully(buf, len)
    return buf.readAllStr
  }

  private static Date readDate(InStream in)
  {
    year  := in.readU2
    month := Month.vals[in.read - 1]
    day   := in.read
    return Date(year, month, day)
  }

  private static Time readTime(InStream in)
  {
    hour  := in.read
    min   := in.read
    sec   := in.read
    nanos := in.readU4
    return Time(hour, min, sec, nanos)
  }

  private static Grid readGrid(InStream in)
  {
    meta    := readDict(in)
    colCnt  := in.readU4
    cols    := Str[,]
    colCnt.times { cols.add(readStr(in)) }
    rowCnt  := in.readU4
    gb      := GridBuilder().setMeta(meta)
    cols.each |c| { gb.addCol(c) }
    rowCnt.times
    {
      row := readDict(in)
      vals := cols.map |c| { row[c] }
      gb.addRow(vals)
    }
    return gb.toGrid
  }

}
