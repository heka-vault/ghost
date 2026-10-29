//! Reading NumPy's `.npy` files and the stored-only `.npz` that `np.savez` writes: Kokoro's
//! voices file (`voices-v1.0.bin`, one `(510, 1, 256)` float32 array per voice) and the
//! parity fixtures `scripts/fixtures/record.py` saves with `np.save`.

use std::{fs, path::Path};

use crate::{InferError, Result};

/// The element types the files Ghost reads use.
#[derive(Debug, Clone, PartialEq)]
pub enum NpyData {
    F32(Vec<f32>),
    F64(Vec<f64>),
    I64(Vec<i64>),
    I32(Vec<i32>),
}

/// One array: its shape (C order) and its elements.
#[derive(Debug, Clone, PartialEq)]
pub struct Npy {
    pub shape: Vec<usize>,
    pub data: NpyData,
}

impl Npy {
    /// The elements as `f32` (a `float64` array is narrowed).
    pub fn to_f32(&self) -> Result<Vec<f32>> {
        match &self.data {
            NpyData::F32(v) => Ok(v.clone()),
            NpyData::F64(v) => Ok(v.iter().map(|x| *x as f32).collect()),
            _ => Err(InferError::Model("expected a float array".into())),
        }
    }

    /// The elements as `f64`.
    pub fn to_f64(&self) -> Result<Vec<f64>> {
        match &self.data {
            NpyData::F32(v) => Ok(v.iter().map(|x| *x as f64).collect()),
            NpyData::F64(v) => Ok(v.clone()),
            _ => Err(InferError::Model("expected a float array".into())),
        }
    }

    /// The elements as `i64`.
    pub fn to_i64(&self) -> Result<Vec<i64>> {
        match &self.data {
            NpyData::I64(v) => Ok(v.clone()),
            NpyData::I32(v) => Ok(v.iter().map(|x| *x as i64).collect()),
            _ => Err(InferError::Model("expected an integer array".into())),
        }
    }

    pub fn len(&self) -> usize {
        self.shape.iter().product()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn bad(what: &str) -> InferError {
    InferError::Model(format!("not a .npy file: {what}"))
}

/// Parse one `.npy` (format 1.0, 2.0 or 3.0, little-endian, C order).
pub fn parse(bytes: &[u8]) -> Result<Npy> {
    if bytes.len() < 10 || &bytes[..6] != b"\x93NUMPY" {
        return Err(bad("magic"));
    }
    let major = bytes[6];
    let (header_len, start) = match major {
        1 => (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10),
        2 | 3 => {
            if bytes.len() < 12 {
                return Err(bad("header length"));
            }
            (
                u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
                12,
            )
        }
        _ => return Err(bad("version")),
    };
    let header = bytes
        .get(start..start + header_len)
        .ok_or_else(|| bad("header length"))?;
    let header = std::str::from_utf8(header).map_err(|_| bad("header text"))?;
    let descr = field(header, "descr").ok_or_else(|| bad("descr"))?;
    let descr = descr.trim_matches(|c| c == '\'' || c == '"');
    if field(header, "fortran_order").map(str::trim) != Some("False") {
        return Err(bad("fortran order"));
    }
    let shape_text = field(header, "shape").ok_or_else(|| bad("shape"))?;
    let shape: Vec<usize> = shape_text
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<usize>().map_err(|_| bad("shape")))
        .collect::<Result<_>>()?;
    let count: usize = shape.iter().product();
    let body = &bytes[start + header_len..];
    let need = |size: usize| -> Result<&[u8]> {
        body.get(..count * size).ok_or_else(|| bad("data length"))
    };
    let data = match descr {
        "<f4" => NpyData::F32(
            need(4)?
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes(*c))
                .collect(),
        ),
        "<f8" => NpyData::F64(
            need(8)?
                .as_chunks::<8>()
                .0
                .iter()
                .map(|c| f64::from_le_bytes(*c))
                .collect(),
        ),
        "<i8" => NpyData::I64(
            need(8)?
                .as_chunks::<8>()
                .0
                .iter()
                .map(|c| i64::from_le_bytes(*c))
                .collect(),
        ),
        "<i4" => NpyData::I32(
            need(4)?
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| i32::from_le_bytes(*c))
                .collect(),
        ),
        other => return Err(bad(&format!("dtype {other}"))),
    };
    Ok(Npy { shape, data })
}

/// The value of `'key': value` in the header dict, as its raw text.
fn field<'a>(header: &'a str, key: &str) -> Option<&'a str> {
    let at = header.find(&format!("'{key}':"))?;
    let rest = header[at + key.len() + 3..].trim_start();
    let end = if rest.starts_with('(') {
        rest.find(')')? + 1
    } else {
        rest.find(',').unwrap_or(rest.len())
    };
    Some(&rest[..end])
}

/// Read one `.npy` file.
pub fn read(path: &Path) -> Result<Npy> {
    parse(&fs::read(path)?)
}

/// Read every member of an uncompressed `.npz` (what `np.savez` writes), by name.
pub fn read_npz_stored(path: &Path) -> Result<Vec<(String, Npy)>> {
    let bytes = fs::read(path)?;
    let zip_err =
        |what: &str| InferError::Model(format!("{}: not a stored .npz: {what}", path.display()));
    let eocd = bytes
        .len()
        .checked_sub(22)
        .and_then(|from| {
            (0..=from)
                .rev()
                .find(|&i| bytes[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])
        })
        .ok_or_else(|| zip_err("no end of central directory"))?;
    let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]) as usize;
    let u32_at = |at: usize| {
        u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize
    };
    let u64_at =
        |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().expect("8 bytes")) as usize;
    let entries = u16_at(eocd + 10);
    let mut at = u32_at(eocd + 16);
    let mut out = Vec::with_capacity(entries);
    for _ in 0..entries {
        if bytes.get(at..at + 46).is_none() || bytes[at..at + 4] != [0x50, 0x4b, 0x01, 0x02] {
            return Err(zip_err("central directory"));
        }
        let method = u16_at(at + 10);
        let mut size = u32_at(at + 24);
        let name_len = u16_at(at + 28);
        let extra_len = u16_at(at + 30);
        let comment_len = u16_at(at + 32);
        let mut local = u32_at(at + 42);
        let name = String::from_utf8_lossy(&bytes[at + 46..at + 46 + name_len]).into_owned();
        // Zip64: the sizes that read 0xFFFFFFFF are in the extra field, in order.
        let extra = &bytes[at + 46 + name_len..at + 46 + name_len + extra_len];
        let mut e = 0;
        while e + 4 <= extra.len() {
            let id = u16::from_le_bytes([extra[e], extra[e + 1]]);
            let len = u16::from_le_bytes([extra[e + 2], extra[e + 3]]) as usize;
            if id == 1 {
                let base = at + 46 + name_len + e + 4;
                let mut off = 0;
                if size == 0xFFFF_FFFF {
                    size = u64_at(base + off);
                    off += 8;
                }
                if u32_at(at + 20) == 0xFFFF_FFFF {
                    off += 8;
                }
                if local == 0xFFFF_FFFF {
                    local = u64_at(base + off);
                }
            }
            e += 4 + len;
        }
        if method != 0 {
            return Err(zip_err(&format!("{name} is compressed")));
        }
        if bytes.get(local..local + 30).is_none()
            || bytes[local..local + 4] != [0x50, 0x4b, 0x03, 0x04]
        {
            return Err(zip_err("local header"));
        }
        let data_at = local + 30 + u16_at(local + 26) + u16_at(local + 28);
        let data = bytes
            .get(data_at..data_at + size)
            .ok_or_else(|| zip_err("member data"))?;
        out.push((name, parse(data)?));
        at += 46 + name_len + extra_len + comment_len;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn npy_bytes(header: &str, body: &[u8]) -> Vec<u8> {
        let mut h = header.as_bytes().to_vec();
        while !(10 + h.len()).is_multiple_of(64) {
            h.push(b' ');
        }
        *h.last_mut().unwrap() = b'\n';
        let mut out = b"\x93NUMPY\x01\x00".to_vec();
        out.extend((h.len() as u16).to_le_bytes());
        out.extend(h);
        out.extend(body);
        out
    }

    #[test]
    fn parses_a_float32_array() {
        let body: Vec<u8> = [1.5f32, -2.0, 3.25, 4.0, 5.0, 6.0]
            .iter()
            .flat_map(|x| x.to_le_bytes())
            .collect();
        let bytes = npy_bytes(
            "{'descr': '<f4', 'fortran_order': False, 'shape': (2, 3), }",
            &body,
        );
        let npy = parse(&bytes).unwrap();
        assert_eq!(npy.shape, vec![2, 3]);
        assert_eq!(npy.to_f32().unwrap(), vec![1.5, -2.0, 3.25, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn parses_a_one_dimensional_int64_array() {
        let body: Vec<u8> = [7i64, 8].iter().flat_map(|x| x.to_le_bytes()).collect();
        let bytes = npy_bytes(
            "{'descr': '<i8', 'fortran_order': False, 'shape': (2,), }",
            &body,
        );
        let npy = parse(&bytes).unwrap();
        assert_eq!(npy.shape, vec![2]);
        assert_eq!(npy.to_i64().unwrap(), vec![7, 8]);
    }

    #[test]
    fn rejects_fortran_order_and_other_dtypes() {
        let bytes = npy_bytes(
            "{'descr': '<f4', 'fortran_order': True, 'shape': (0,), }",
            &[],
        );
        assert!(parse(&bytes).is_err());
        let bytes = npy_bytes(
            "{'descr': '<u1', 'fortran_order': False, 'shape': (0,), }",
            &[],
        );
        assert!(parse(&bytes).is_err());
    }

    #[test]
    fn reads_a_stored_npz() {
        // A two-member zip written by hand: stored entries, no zip64.
        let member = |name: &str, data: &[u8]| -> (Vec<u8>, Vec<u8>) {
            let mut local = vec![0x50, 0x4b, 0x03, 0x04, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0];
            local.extend(0u32.to_le_bytes()); // crc (unchecked)
            local.extend((data.len() as u32).to_le_bytes());
            local.extend((data.len() as u32).to_le_bytes());
            local.extend((name.len() as u16).to_le_bytes());
            local.extend(0u16.to_le_bytes());
            local.extend(name.as_bytes());
            local.extend(data);
            let mut central = vec![0x50, 0x4b, 0x01, 0x02, 20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0];
            central.extend(0u32.to_le_bytes());
            central.extend((data.len() as u32).to_le_bytes());
            central.extend((data.len() as u32).to_le_bytes());
            central.extend((name.len() as u16).to_le_bytes());
            central.extend(0u16.to_le_bytes());
            central.extend(0u16.to_le_bytes());
            central.extend([0, 0, 0, 0, 0, 0, 0, 0]); // disk, attrs
            central.extend(0u32.to_le_bytes()); // local offset, patched below
            central.extend(name.as_bytes());
            (local, central)
        };
        let a = npy_bytes(
            "{'descr': '<f4', 'fortran_order': False, 'shape': (1,), }",
            &1.0f32.to_le_bytes(),
        );
        let b = npy_bytes(
            "{'descr': '<f4', 'fortran_order': False, 'shape': (1,), }",
            &2.0f32.to_le_bytes(),
        );
        let (la, mut ca) = member("a.npy", &a);
        let (lb, mut cb) = member("b.npy", &b);
        ca[42..46].copy_from_slice(&0u32.to_le_bytes());
        cb[42..46].copy_from_slice(&(la.len() as u32).to_le_bytes());
        let mut zip = la.clone();
        zip.extend(&lb);
        let cd_at = zip.len();
        zip.extend(&ca);
        zip.extend(&cb);
        let cd_len = ca.len() + cb.len();
        zip.extend([0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0, 2, 0, 2, 0]);
        zip.extend((cd_len as u32).to_le_bytes());
        zip.extend((cd_at as u32).to_le_bytes());
        zip.extend(0u16.to_le_bytes());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("voices.bin");
        fs::write(&path, zip).unwrap();
        let members = read_npz_stored(&path).unwrap();
        assert_eq!(members.len(), 2);
        assert_eq!(members[0].0, "a.npy");
        assert_eq!(members[0].1.to_f32().unwrap(), vec![1.0]);
        assert_eq!(members[1].1.to_f32().unwrap(), vec![2.0]);
    }
}
