//! Just enough glTF 2.0 for avatars: a `.glb`'s JSON and binary chunk, accessors (normalized
//! integers and sparse ones included) and embedded images. External buffers and images aren't
//! supported (VRM files never use them).

use serde_json::Value;

use crate::Error;

pub struct Glb {
    pub json: Value,
    pub bin: Vec<u8>,
}

impl Glb {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        let u32_at = |at: usize| -> Result<u32, Error> {
            bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or(Error::Invalid("truncated file"))
        };
        if bytes.get(..4) != Some(b"glTF") {
            return Err(Error::Invalid("not a glTF binary (.glb/.vrm)"));
        }
        if u32_at(4)? != 2 {
            return Err(Error::Invalid("only glTF 2.0 is supported"));
        }
        let total = (u32_at(8)? as usize).min(bytes.len());
        let (mut json, mut bin) = (None, Vec::new());
        let mut at = 12;
        while at + 8 <= total {
            let len = u32_at(at)? as usize;
            let kind = u32_at(at + 4)?;
            let data = bytes.get(at + 8..at + 8 + len).ok_or(Error::Invalid("truncated chunk"))?;
            match kind {
                0x4E4F534A => json = Some(serde_json::from_slice(data).map_err(|_| Error::Invalid("unreadable glTF JSON"))?),
                0x004E4942 => bin = data.to_vec(),
                _ => {}
            }
            at += 8 + len.div_ceil(4) * 4;
        }
        Ok(Glb { json: json.ok_or(Error::Invalid("no glTF JSON"))?, bin })
    }

    fn view(&self, index: usize) -> Result<(&[u8], usize), Error> {
        let v = &self.json["bufferViews"][index];
        if v["buffer"].as_u64().unwrap_or(0) != 0 {
            return Err(Error::Invalid("external buffers aren't supported"));
        }
        let offset = v["byteOffset"].as_u64().unwrap_or(0) as usize;
        let len = v["byteLength"].as_u64().ok_or(Error::Invalid("buffer view without a length"))? as usize;
        let stride = v["byteStride"].as_u64().unwrap_or(0) as usize;
        let data = self.bin.get(offset..offset + len).ok_or(Error::Invalid("buffer view out of range"))?;
        Ok((data, stride))
    }

    /// The bytes of an embedded image (PNG or JPEG).
    pub fn image_bytes(&self, index: usize) -> Result<&[u8], Error> {
        let view = self.json["images"][index]["bufferView"].as_u64().ok_or(Error::Invalid("external images aren't supported"))?;
        Ok(self.view(view as usize)?.0)
    }

    /// An accessor's elements as f32s (`n` components each), normalized integers mapped to 0..1
    /// (or -1..1), sparse values applied.
    pub fn floats(&self, accessor: usize) -> Result<Vec<f32>, Error> {
        let a = &self.json["accessors"][accessor];
        let count = a["count"].as_u64().ok_or(Error::Invalid("accessor without a count"))? as usize;
        let n = components(a["type"].as_str().unwrap_or(""))?;
        let ct = a["componentType"].as_u64().unwrap_or(0);
        let normalized = a["normalized"].as_bool().unwrap_or(false);
        let mut out = vec![0.0f32; count * n];
        if let Some(view) = a["bufferView"].as_u64() {
            let (data, stride) = self.view(view as usize)?;
            let offset = a["byteOffset"].as_u64().unwrap_or(0) as usize;
            read(data, offset, stride, count, n, ct, normalized, &mut out)?;
        }
        if let Some(sparse) = a.get("sparse") {
            let k = sparse["count"].as_u64().unwrap_or(0) as usize;
            let idx = &sparse["indices"];
            let (idata, _) = self.view(idx["bufferView"].as_u64().ok_or(Error::Invalid("sparse indices"))? as usize)?;
            let mut indices = vec![0.0f32; k];
            read(idata, idx["byteOffset"].as_u64().unwrap_or(0) as usize, 0, k, 1, idx["componentType"].as_u64().unwrap_or(0), false, &mut indices)?;
            let val = &sparse["values"];
            let (vdata, _) = self.view(val["bufferView"].as_u64().ok_or(Error::Invalid("sparse values"))? as usize)?;
            let mut values = vec![0.0f32; k * n];
            read(vdata, val["byteOffset"].as_u64().unwrap_or(0) as usize, 0, k, n, ct, normalized, &mut values)?;
            for (j, &i) in indices.iter().enumerate() {
                let i = i as usize;
                if i < count {
                    out[i * n..i * n + n].copy_from_slice(&values[j * n..j * n + n]);
                }
            }
        }
        Ok(out)
    }

    /// An accessor's elements as integers (indices, joints).
    pub fn ints(&self, accessor: usize) -> Result<Vec<u32>, Error> {
        let a = &self.json["accessors"][accessor];
        let count = a["count"].as_u64().ok_or(Error::Invalid("accessor without a count"))? as usize;
        let n = components(a["type"].as_str().unwrap_or(""))?;
        let ct = a["componentType"].as_u64().unwrap_or(0);
        let Some(view) = a["bufferView"].as_u64() else { return Ok(vec![0; count * n]) };
        let (data, stride) = self.view(view as usize)?;
        let offset = a["byteOffset"].as_u64().unwrap_or(0) as usize;
        let size = component_size(ct)?;
        let stride = if stride == 0 { size * n } else { stride };
        let mut out = Vec::with_capacity(count * n);
        for e in 0..count {
            for c in 0..n {
                let at = offset + e * stride + c * size;
                let b = data.get(at..at + size).ok_or(Error::Invalid("accessor out of range"))?;
                out.push(match ct {
                    5121 => b[0] as u32,
                    5123 => u16::from_le_bytes([b[0], b[1]]) as u32,
                    5125 => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                    _ => return Err(Error::Invalid("integer accessor of an unexpected type")),
                });
            }
        }
        Ok(out)
    }
}

fn components(kind: &str) -> Result<usize, Error> {
    Ok(match kind {
        "SCALAR" => 1,
        "VEC2" => 2,
        "VEC3" => 3,
        "VEC4" | "MAT2" => 4,
        "MAT3" => 9,
        "MAT4" => 16,
        _ => return Err(Error::Invalid("accessor of an unknown type")),
    })
}

fn component_size(ct: u64) -> Result<usize, Error> {
    Ok(match ct {
        5120 | 5121 => 1,
        5122 | 5123 => 2,
        5125 | 5126 => 4,
        _ => return Err(Error::Invalid("accessor of an unknown component type")),
    })
}

#[allow(clippy::too_many_arguments)]
fn read(data: &[u8], offset: usize, stride: usize, count: usize, n: usize, ct: u64, normalized: bool, out: &mut [f32]) -> Result<(), Error> {
    let size = component_size(ct)?;
    let stride = if stride == 0 { size * n } else { stride };
    for e in 0..count {
        for c in 0..n {
            let at = offset + e * stride + c * size;
            let b = data.get(at..at + size).ok_or(Error::Invalid("accessor out of range"))?;
            out[e * n + c] = match (ct, normalized) {
                (5126, _) => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                (5120, true) => (b[0] as i8 as f32 / 127.0).max(-1.0),
                (5121, true) => b[0] as f32 / 255.0,
                (5122, true) => (i16::from_le_bytes([b[0], b[1]]) as f32 / 32767.0).max(-1.0),
                (5123, true) => u16::from_le_bytes([b[0], b[1]]) as f32 / 65535.0,
                (5120, false) => b[0] as i8 as f32,
                (5121, false) => b[0] as f32,
                (5122, false) => i16::from_le_bytes([b[0], b[1]]) as f32,
                (5123, false) => u16::from_le_bytes([b[0], b[1]]) as f32,
                (5125, _) => u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32,
                _ => return Err(Error::Invalid("accessor of an unknown component type")),
            };
        }
    }
    Ok(())
}
