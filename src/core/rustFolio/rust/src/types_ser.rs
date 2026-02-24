// types_ser.rs — Serialize Rust types to wire bytes.
//
// Type tag bytes:
//   0x00 Null, 0x01 Marker, 0x02 NA, 0x03 Remove/None,
//   0x04 Bool, 0x05 Number, 0x06 Str, 0x07 Ref, 0x08 Uri,
//   0x09 Date, 0x0A Time, 0x0B DateTime, 0x0C Coord,
//   0x0D XStr, 0x0E Symbol, 0x0F Span,
//   0x10 List, 0x11 Dict, 0x12 Grid

use crate::types::*;
use crate::protocol::*;
use chrono::Datelike;
use chrono::Timelike;

pub mod type_tag {
    pub const NULL:     u8 = 0x00;
    pub const MARKER:   u8 = 0x01;
    pub const NA:       u8 = 0x02;
    pub const REMOVE:   u8 = 0x03;
    pub const BOOL:     u8 = 0x04;
    pub const NUMBER:   u8 = 0x05;
    pub const STR:      u8 = 0x06;
    pub const REF:      u8 = 0x07;
    pub const URI:      u8 = 0x08;
    pub const DATE:     u8 = 0x09;
    pub const TIME:     u8 = 0x0A;
    pub const DATETIME: u8 = 0x0B;
    pub const COORD:    u8 = 0x0C;
    pub const XSTR:     u8 = 0x0D;
    pub const SYMBOL:   u8 = 0x0E;
    pub const SPAN:     u8 = 0x0F;
    pub const LIST:     u8 = 0x10;
    pub const DICT:     u8 = 0x11;
    pub const GRID:     u8 = 0x12;
}

/// Serialize a Val to a byte buffer.
pub fn write_val(buf: &mut Vec<u8>, val: &Val) {
    match val {
        Val::Null        => write_u8(buf, type_tag::NULL),
        Val::Marker      => write_u8(buf, type_tag::MARKER),
        Val::NA          => write_u8(buf, type_tag::NA),
        Val::Remove      => write_u8(buf, type_tag::REMOVE),

        Val::Bool(b) => {
            write_u8(buf, type_tag::BOOL);
            write_u8(buf, if *b { 1 } else { 0 });
        }

        Val::Number(n, unit) => {
            write_u8(buf, type_tag::NUMBER);
            write_f64(buf, *n);
            write_str_to(buf, unit.as_deref().unwrap_or(""));
        }

        Val::Str(s) => {
            write_u8(buf, type_tag::STR);
            write_str_to(buf, s);
        }

        Val::Ref(r) => {
            write_u8(buf, type_tag::REF);
            write_href(buf, r);
        }

        Val::Uri(u) => {
            write_u8(buf, type_tag::URI);
            write_str_to(buf, u);
        }

        Val::Date(d) => {
            write_u8(buf, type_tag::DATE);
            // 2 bytes year + 1 byte month + 1 byte day
            write_u16(buf, d.year() as u16);
            write_u8(buf, d.month() as u8);
            write_u8(buf, d.day() as u8);
        }

        Val::Time(t) => {
            write_u8(buf, type_tag::TIME);
            // 1 hour + 1 min + 1 sec + 4 nanos
            write_u8(buf, t.hour() as u8);
            write_u8(buf, t.minute() as u8);
            write_u8(buf, t.second() as u8);
            write_u32(buf, t.nanosecond());
        }

        Val::DateTime(dt) => {
            write_u8(buf, type_tag::DATETIME);
            write_datetime(buf, dt);
        }

        Val::Coord(lat, lng) => {
            write_u8(buf, type_tag::COORD);
            write_f64(buf, *lat);
            write_f64(buf, *lng);
        }

        Val::XStr(ty, v) => {
            write_u8(buf, type_tag::XSTR);
            write_str_to(buf, ty);
            write_str_to(buf, v);
        }

        Val::Symbol(s) => {
            write_u8(buf, type_tag::SYMBOL);
            write_str_to(buf, s);
        }

        Val::Span(s) => {
            write_u8(buf, type_tag::SPAN);
            write_str_to(buf, s);
        }

        Val::List(items) => {
            write_u8(buf, type_tag::LIST);
            write_u32(buf, items.len() as u32);
            for item in items {
                write_val(buf, item);
            }
        }

        Val::Dict(d) => {
            write_u8(buf, type_tag::DICT);
            write_dict(buf, d);
        }

        Val::Grid(g) => {
            write_u8(buf, type_tag::GRID);
            write_grid(buf, g);
        }
    }
}

/// Serialize an HRef.
pub fn write_href(buf: &mut Vec<u8>, r: &HRef) {
    write_str_to(buf, &r.id);
    write_str_to(buf, r.dis.as_deref().unwrap_or(""));
}

/// Serialize a Dict (4-byte count + sorted name/value pairs).
pub fn write_dict(buf: &mut Vec<u8>, dict: &Dict) {
    write_u32(buf, dict.tags.len() as u32);
    for (name, val) in &dict.tags {
        write_str_to(buf, name);
        write_val(buf, val);
    }
}

/// Serialize an optional Dict: 0x00 = null, 0x01 = present.
pub fn write_opt_dict(buf: &mut Vec<u8>, dict: Option<&Dict>) {
    match dict {
        None    => write_u8(buf, 0x00),
        Some(d) => { write_u8(buf, 0x01); write_dict(buf, d); }
    }
}

/// Serialize an optional DateTime: 0x00 = null, 0x01 = present.
pub fn write_opt_datetime(buf: &mut Vec<u8>, dt: Option<&chrono::DateTime<chrono_tz::Tz>>) {
    match dt {
        None     => write_u8(buf, 0x00),
        Some(dt) => { write_u8(buf, 0x01); write_datetime(buf, dt); }
    }
}

pub fn write_datetime(buf: &mut Vec<u8>, dt: &chrono::DateTime<chrono_tz::Tz>) {
    use chrono::Datelike;
    use chrono::Timelike;
    // Date part
    write_u16(buf, dt.year() as u16);
    write_u8(buf, dt.month() as u8);
    write_u8(buf, dt.day() as u8);
    // Time part
    write_u8(buf, dt.hour() as u8);
    write_u8(buf, dt.minute() as u8);
    write_u8(buf, dt.second() as u8);
    write_u32(buf, dt.nanosecond());
    // Timezone name
    let tz_name = dt.timezone().name();
    write_str_to(buf, tz_name);
}

fn write_grid(buf: &mut Vec<u8>, g: &Grid) {
    write_dict(buf, &g.meta);
    write_u32(buf, g.cols.len() as u32);
    for col in &g.cols {
        write_str_to(buf, col);
    }
    write_u32(buf, g.rows.len() as u32);
    for row in &g.rows {
        write_dict(buf, row);
    }
}

