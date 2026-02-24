// types_de.rs — Deserialize wire bytes to Rust types.

use crate::types::*;
use crate::protocol::*;
use crate::types_ser::type_tag;
use crate::error::{FolioError, Result};
use chrono::{NaiveDate, NaiveTime, TimeZone};

/// Deserialize a Val from a byte slice at the given position.
pub fn read_val(data: &[u8], pos: &mut usize) -> Result<Val> {
    let tag = read_u8(data, pos)?;
    match tag {
        type_tag::NULL   => Ok(Val::Null),
        type_tag::MARKER => Ok(Val::Marker),
        type_tag::NA     => Ok(Val::NA),
        type_tag::REMOVE => Ok(Val::Remove),

        type_tag::BOOL => {
            let b = read_u8(data, pos)?;
            Ok(Val::Bool(b != 0))
        }

        type_tag::NUMBER => {
            let n    = read_f64(data, pos)?;
            let unit = read_str_from(data, pos)?;
            Ok(Val::Number(n, if unit.is_empty() { None } else { Some(unit) }))
        }

        type_tag::STR => {
            let s = read_str_from(data, pos)?;
            Ok(Val::Str(s))
        }

        type_tag::REF => {
            let r = read_href(data, pos)?;
            Ok(Val::Ref(r))
        }

        type_tag::URI => {
            let u = read_str_from(data, pos)?;
            Ok(Val::Uri(u))
        }

        type_tag::DATE => {
            let year  = read_u16(data, pos)? as i32;
            let month = read_u8(data, pos)? as u32;
            let day   = read_u8(data, pos)? as u32;
            let d = NaiveDate::from_ymd_opt(year, month, day)
                .ok_or_else(|| FolioError::Protocol(format!("Invalid date: {}-{}-{}", year, month, day)))?;
            Ok(Val::Date(d))
        }

        type_tag::TIME => {
            let hour  = read_u8(data, pos)? as u32;
            let min   = read_u8(data, pos)? as u32;
            let sec   = read_u8(data, pos)? as u32;
            let nanos = read_u32(data, pos)?;
            let t = NaiveTime::from_hms_nano_opt(hour, min, sec, nanos)
                .ok_or_else(|| FolioError::Protocol(format!("Invalid time: {}:{}:{}.{}", hour, min, sec, nanos)))?;
            Ok(Val::Time(t))
        }

        type_tag::DATETIME => {
            let dt = read_datetime(data, pos)?;
            Ok(Val::DateTime(dt))
        }

        type_tag::COORD => {
            let lat = read_f64(data, pos)?;
            let lng = read_f64(data, pos)?;
            Ok(Val::Coord(lat, lng))
        }

        type_tag::XSTR => {
            let ty  = read_str_from(data, pos)?;
            let val = read_str_from(data, pos)?;
            Ok(Val::XStr(ty, val))
        }

        type_tag::SYMBOL => {
            let s = read_str_from(data, pos)?;
            Ok(Val::Symbol(s))
        }

        type_tag::SPAN => {
            let s = read_str_from(data, pos)?;
            Ok(Val::Span(s))
        }

        type_tag::LIST => {
            let count = read_u32(data, pos)? as usize;
            let mut items = Vec::with_capacity(count);
            for _ in 0..count {
                items.push(read_val(data, pos)?);
            }
            Ok(Val::List(items))
        }

        type_tag::DICT => {
            let d = read_dict(data, pos)?;
            Ok(Val::Dict(d))
        }

        type_tag::GRID => {
            let g = read_grid(data, pos)?;
            Ok(Val::Grid(g))
        }

        other => Err(FolioError::Protocol(format!("Unknown type tag: {:#04x}", other))),
    }
}

/// Deserialize an HRef.
pub fn read_href(data: &[u8], pos: &mut usize) -> Result<HRef> {
    let id  = read_str_from(data, pos)?;
    let dis = read_str_from(data, pos)?;
    Ok(HRef {
        id,
        dis: if dis.is_empty() { None } else { Some(dis) },
    })
}

/// Deserialize a Dict.
pub fn read_dict(data: &[u8], pos: &mut usize) -> Result<Dict> {
    let count = read_u32(data, pos)? as usize;
    let mut tags = Vec::with_capacity(count);
    for _ in 0..count {
        let name = read_str_from(data, pos)?;
        let val  = read_val(data, pos)?;
        tags.push((name, val));
    }
    // Tags should arrive sorted, but sort defensively
    tags.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(Dict { tags })
}

/// Deserialize a DateTime.
pub fn read_datetime(data: &[u8], pos: &mut usize) -> Result<chrono::DateTime<chrono_tz::Tz>> {
    let year  = read_u16(data, pos)? as i32;
    let month = read_u8(data, pos)? as u32;
    let day   = read_u8(data, pos)? as u32;
    let hour  = read_u8(data, pos)? as u32;
    let min   = read_u8(data, pos)? as u32;
    let sec   = read_u8(data, pos)? as u32;
    let nanos = read_u32(data, pos)?;
    let tz_name = read_str_from(data, pos)?;

    let tz: chrono_tz::Tz = tz_name.parse()
        .map_err(|_| FolioError::Protocol(format!("Unknown timezone: {}", tz_name)))?;

    let naive = NaiveDate::from_ymd_opt(year, month, day)
        .and_then(|d| d.and_hms_nano_opt(hour, min, sec, nanos))
        .ok_or_else(|| FolioError::Protocol("Invalid datetime values".into()))?;

    // Use from_local_datetime, which can be ambiguous — pick the earliest
    tz.from_local_datetime(&naive)
        .earliest()
        .ok_or_else(|| FolioError::Protocol(format!("Ambiguous datetime for timezone {}", tz_name)))
}

/// Deserialize an optional DateTime (0x00 = null, 0x01 = present).
pub fn read_opt_datetime(data: &[u8], pos: &mut usize) -> Result<Option<chrono::DateTime<chrono_tz::Tz>>> {
    let flag = read_u8(data, pos)?;
    if flag == 0x00 {
        Ok(None)
    } else {
        Ok(Some(read_datetime(data, pos)?))
    }
}

fn read_grid(data: &[u8], pos: &mut usize) -> Result<Grid> {
    let meta    = read_dict(data, pos)?;
    let col_cnt = read_u32(data, pos)? as usize;
    let mut cols = Vec::with_capacity(col_cnt);
    for _ in 0..col_cnt {
        cols.push(read_str_from(data, pos)?);
    }
    let row_cnt = read_u32(data, pos)? as usize;
    let mut rows = Vec::with_capacity(row_cnt);
    for _ in 0..row_cnt {
        rows.push(read_dict(data, pos)?);
    }
    Ok(Grid { meta, cols, rows })
}

/// Deserialize a Diff from the wire payload.
pub fn read_diff(data: &[u8], pos: &mut usize) -> Result<Diff> {
    let flags   = read_u8(data, pos)?;
    let id      = read_href(data, pos)?;
    let old_mod = read_opt_datetime(data, pos)?;
    let changes = read_dict(data, pos)?;
    Ok(Diff { id, old_mod, changes, flags })
}
