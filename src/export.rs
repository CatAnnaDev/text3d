use std::fmt;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;

use crate::render::{MeshGroup, SceneMesh};

const DEFAULT_COLOR: [u8; 4] = [222, 222, 226, 255];
const MAX_STORED_BLOCK: usize = 65535;
const PNG_SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

static FILTER_NONE: [u8; 1] = [0];
static CRC_TABLE: [u32; 256] = build_crc_table();

pub fn write_obj(path: &Path, mesh: &SceneMesh) -> Result<String, String> {
    let groups = prepare_groups(mesh)?;

    let material_path = path.with_extension("mtl");
    let material_name = material_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("text3d.mtl");

    let material_file = File::create(&material_path).map_err(|err| failure(&material_path, err))?;
    let mut material_out = BufWriter::with_capacity(1 << 15, material_file);
    write_mtl_body(&mut material_out, &groups)
        .and_then(|()| material_out.flush())
        .map_err(|err| failure(&material_path, err))?;

    let file = File::create(path).map_err(|err| failure(path, err))?;
    let mut out = BufWriter::with_capacity(1 << 16, file);
    write_obj_body(&mut out, mesh, &groups, material_name)
        .and_then(|()| out.flush())
        .map_err(|err| failure(path, err))?;

    Ok(format!("exporte {}", path.display()))
}

pub fn write_glb(path: &Path, mesh: &SceneMesh) -> Result<String, String> {
    let groups = prepare_groups(mesh)?;

    let position_bytes = mesh.vertices.len() * 12;
    let index_bytes = mesh.indices.len() * 4;
    let binary_length = position_bytes * 2 + index_bytes;
    if binary_length > u32::MAX as usize {
        return Err(String::from("maillage trop grand pour un glb"));
    }

    let json = build_gltf_json(
        mesh,
        &groups,
        position_bytes as u32,
        index_bytes as u32,
        binary_length as u32,
    );
    if binary_length as u64 + json.len() as u64 + 32 > u32::MAX as u64 {
        return Err(String::from("maillage trop grand pour un glb"));
    }

    let file = File::create(path).map_err(|err| failure(path, err))?;
    let mut out = BufWriter::with_capacity(1 << 16, file);
    write_glb_body(&mut out, mesh, &json, binary_length as u32)
        .and_then(|()| out.flush())
        .map_err(|err| failure(path, err))?;

    Ok(format!("exporte {}", path.display()))
}

pub fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<String, String> {
    if width == 0 || height == 0 {
        return Err(String::from("image vide"));
    }
    let row_bytes = width as usize * 4;
    let Some(raw_length) = (row_bytes + 1).checked_mul(height as usize) else {
        return Err(String::from("image trop grande"));
    };
    let pixel_bytes = row_bytes * height as usize;
    if rgba.len() < pixel_bytes {
        return Err(String::from("donnees image incompletes"));
    }

    let block_count = raw_length.div_ceil(MAX_STORED_BLOCK).max(1);
    let stream_length = 2 + block_count * 5 + raw_length + 4;
    if stream_length > u32::MAX as usize {
        return Err(String::from("image trop grande"));
    }

    let file = File::create(path).map_err(|err| failure(path, err))?;
    let mut out = BufWriter::with_capacity(1 << 16, file);
    write_png_body(
        &mut out,
        width,
        height,
        &rgba[..pixel_bytes],
        raw_length,
        stream_length as u32,
    )
    .and_then(|()| out.flush())
    .map_err(|err| failure(path, err))?;

    Ok(format!("exporte {}", path.display()))
}

fn failure(path: &Path, err: io::Error) -> String {
    format!("echec ecriture {}: {err}", path.display())
}

fn prepare_groups(mesh: &SceneMesh) -> Result<Vec<MeshGroup>, String> {
    if mesh.vertices.is_empty() || mesh.indices.len() < 3 {
        return Err(String::from("rien a exporter"));
    }
    if mesh.normals.len() != mesh.vertices.len() {
        return Err(String::from("maillage incoherent: normales manquantes"));
    }
    if mesh.vertices.len() > u32::MAX as usize || mesh.indices.len() > u32::MAX as usize {
        return Err(String::from("maillage trop grand"));
    }

    let vertex_limit = mesh.vertices.len() as u32;
    if mesh.indices.iter().any(|&index| index >= vertex_limit) {
        return Err(String::from("maillage incoherent: indice hors bornes"));
    }

    let total = mesh.indices.len() as u32;
    let mut groups = Vec::with_capacity(mesh.groups.len().max(1));
    for group in &mesh.groups {
        if group.start >= total {
            continue;
        }
        let available = total - group.start;
        let usable = group.count.min(available);
        let count = usable - usable % 3;
        if count == 0 {
            continue;
        }
        groups.push(MeshGroup {
            color: group.color,
            start: group.start,
            count,
        });
    }
    if groups.is_empty() {
        groups.push(MeshGroup {
            color: DEFAULT_COLOR,
            start: 0,
            count: total - total % 3,
        });
    }
    Ok(groups)
}

fn write_obj_body<W: Write>(
    out: &mut W,
    mesh: &SceneMesh,
    groups: &[MeshGroup],
    material_name: &str,
) -> io::Result<()> {
    writeln!(out, "mtllib {material_name}")?;
    writeln!(out, "o texte")?;
    for vertex in &mesh.vertices {
        writeln!(out, "v {:.6} {:.6} {:.6}", vertex[0], vertex[1], vertex[2])?;
    }
    for normal in &mesh.normals {
        writeln!(out, "vn {:.6} {:.6} {:.6}", normal[0], normal[1], normal[2])?;
    }
    for (slot, group) in groups.iter().enumerate() {
        writeln!(out, "usemtl couleur{slot}")?;
        let start = group.start as usize;
        let end = start + group.count as usize;
        for triangle in mesh.indices[start..end].as_chunks::<3>().0 {
            let first = triangle[0] + 1;
            let second = triangle[1] + 1;
            let third = triangle[2] + 1;
            writeln!(
                out,
                "f {first}//{first} {second}//{second} {third}//{third}"
            )?;
        }
    }
    Ok(())
}

fn write_mtl_body<W: Write>(out: &mut W, groups: &[MeshGroup]) -> io::Result<()> {
    for (slot, group) in groups.iter().enumerate() {
        let color = group.color;
        let red = srgb_to_linear(color[0]);
        let green = srgb_to_linear(color[1]);
        let blue = srgb_to_linear(color[2]);
        let alpha = color[3] as f32 / 255.0;
        writeln!(out, "newmtl couleur{slot}")?;
        writeln!(out, "Ka 0.000000 0.000000 0.000000")?;
        writeln!(out, "Kd {red:.6} {green:.6} {blue:.6}")?;
        writeln!(out, "Ks 0.040000 0.040000 0.040000")?;
        writeln!(out, "Ns 40.000000")?;
        writeln!(out, "d {alpha:.6}")?;
        writeln!(out, "illum 2")?;
    }
    Ok(())
}

fn build_gltf_json(
    mesh: &SceneMesh,
    groups: &[MeshGroup],
    position_bytes: u32,
    index_bytes: u32,
    binary_length: u32,
) -> String {
    let mut minimum = [f32::INFINITY; 3];
    let mut maximum = [f32::NEG_INFINITY; 3];
    for vertex in &mesh.vertices {
        for axis in 0..3 {
            let value = sanitize(vertex[axis]);
            if value < minimum[axis] {
                minimum[axis] = value;
            }
            if value > maximum[axis] {
                maximum[axis] = value;
            }
        }
    }

    let vertex_count = mesh.vertices.len();
    let mut json = String::with_capacity(768 + groups.len() * 448);
    json.push_str(
        "{\"asset\":{\"version\":\"2.0\",\"generator\":\"text3d\"},\"scene\":0,\
         \"scenes\":[{\"nodes\":[0]}],\"nodes\":[{\"mesh\":0,\"name\":\"texte\"}],\
         \"meshes\":[{\"name\":\"texte\",\"primitives\":[",
    );
    for slot in 0..groups.len() {
        if slot > 0 {
            json.push(',');
        }
        push_json(
            &mut json,
            format_args!(
                "{{\"attributes\":{{\"POSITION\":0,\"NORMAL\":1}},\"indices\":{},\"material\":{},\"mode\":4}}",
                slot + 2,
                slot
            ),
        );
    }

    json.push_str("]}],\"materials\":[");
    for (slot, group) in groups.iter().enumerate() {
        if slot > 0 {
            json.push(',');
        }
        let color = group.color;
        push_json(
            &mut json,
            format_args!(
                "{{\"name\":\"couleur{}\",\"pbrMetallicRoughness\":{{\"baseColorFactor\":[{:.6},{:.6},{:.6},{:.6}],\"metallicFactor\":0.1,\"roughnessFactor\":0.45}}",
                slot,
                srgb_to_linear(color[0]),
                srgb_to_linear(color[1]),
                srgb_to_linear(color[2]),
                color[3] as f32 / 255.0
            ),
        );
        if color[3] < 255 {
            json.push_str(",\"alphaMode\":\"BLEND\"");
        }
        json.push('}');
    }

    json.push_str("],\"accessors\":[");
    push_json(
        &mut json,
        format_args!(
            "{{\"bufferView\":0,\"componentType\":5126,\"count\":{},\"type\":\"VEC3\",\"min\":[{},{},{}],\"max\":[{},{},{}]}},",
            vertex_count, minimum[0], minimum[1], minimum[2], maximum[0], maximum[1], maximum[2]
        ),
    );
    push_json(
        &mut json,
        format_args!(
            "{{\"bufferView\":1,\"componentType\":5126,\"count\":{vertex_count},\"type\":\"VEC3\"}}"
        ),
    );
    for group in groups {
        push_json(
            &mut json,
            format_args!(
                ",{{\"bufferView\":2,\"byteOffset\":{},\"componentType\":5125,\"count\":{},\"type\":\"SCALAR\"}}",
                group.start * 4,
                group.count
            ),
        );
    }

    push_json(
        &mut json,
        format_args!(
            "],\"bufferViews\":[{{\"buffer\":0,\"byteOffset\":0,\"byteLength\":{},\"target\":34962}},\
             {{\"buffer\":0,\"byteOffset\":{},\"byteLength\":{},\"target\":34962}},\
             {{\"buffer\":0,\"byteOffset\":{},\"byteLength\":{},\"target\":34963}}],\
             \"buffers\":[{{\"byteLength\":{}}}]}}",
            position_bytes,
            position_bytes,
            position_bytes,
            position_bytes * 2,
            index_bytes,
            binary_length
        ),
    );
    json
}

fn write_glb_body<W: Write>(
    out: &mut W,
    mesh: &SceneMesh,
    json: &str,
    binary_length: u32,
) -> io::Result<()> {
    let json_padding = (4 - json.len() % 4) % 4;
    let json_chunk = json.len() + json_padding;
    let binary_padding = (4 - binary_length as usize % 4) % 4;
    let binary_chunk = binary_length as usize + binary_padding;
    let total = 12 + 8 + json_chunk + 8 + binary_chunk;

    out.write_all(b"glTF")?;
    out.write_all(&2u32.to_le_bytes())?;
    out.write_all(&(total as u32).to_le_bytes())?;

    out.write_all(&(json_chunk as u32).to_le_bytes())?;
    out.write_all(b"JSON")?;
    out.write_all(json.as_bytes())?;
    out.write_all(&[0x20; 3][..json_padding])?;

    out.write_all(&(binary_chunk as u32).to_le_bytes())?;
    out.write_all(&[0x42, 0x49, 0x4E, 0x00])?;
    write_vec3_array(out, &mesh.vertices)?;
    write_vec3_array(out, &mesh.normals)?;
    write_u32_array(out, &mesh.indices)?;
    out.write_all(&[0u8; 3][..binary_padding])
}

fn write_png_body<W: Write>(
    out: &mut W,
    width: u32,
    height: u32,
    rgba: &[u8],
    raw_length: usize,
    stream_length: u32,
) -> io::Result<()> {
    out.write_all(&PNG_SIGNATURE)?;

    let mut header = ChunkWriter::begin(out, 13, b"IHDR")?;
    header.push(&width.to_be_bytes())?;
    header.push(&height.to_be_bytes())?;
    header.push(&[8, 6, 0, 0, 0])?;
    header.finish()?;

    let mut data = ChunkWriter::begin(out, stream_length, b"IDAT")?;
    data.push(&[0x78, 0x01])?;

    let mut source = ScanlineSource {
        rgba,
        row_bytes: width as usize * 4,
        row: 0,
        offset: 0,
    };
    let mut adler = Adler32::new();
    let mut remaining = raw_length;
    loop {
        let block = remaining.min(MAX_STORED_BLOCK);
        let length = block as u16;
        let complement = !length;
        data.push(&[
            u8::from(remaining == block),
            length as u8,
            (length >> 8) as u8,
            complement as u8,
            (complement >> 8) as u8,
        ])?;
        let mut left = block;
        while left > 0 {
            let slice = source.next_slice(left);
            adler.update(slice);
            data.push(slice)?;
            left -= slice.len();
        }
        remaining -= block;
        if remaining == 0 {
            break;
        }
    }
    data.push(&adler.value().to_be_bytes())?;
    data.finish()?;

    ChunkWriter::begin(out, 0, b"IEND")?.finish()
}

struct ChunkWriter<'a, W: Write> {
    out: &'a mut W,
    crc: u32,
}

impl<'a, W: Write> ChunkWriter<'a, W> {
    fn begin(out: &'a mut W, length: u32, kind: &[u8; 4]) -> io::Result<ChunkWriter<'a, W>> {
        out.write_all(&length.to_be_bytes())?;
        out.write_all(kind)?;
        let crc = update_crc(0xFFFF_FFFF, kind);
        Ok(ChunkWriter { out, crc })
    }

    fn push(&mut self, data: &[u8]) -> io::Result<()> {
        self.out.write_all(data)?;
        self.crc = update_crc(self.crc, data);
        Ok(())
    }

    fn finish(self) -> io::Result<()> {
        self.out.write_all(&(self.crc ^ 0xFFFF_FFFF).to_be_bytes())
    }
}

struct ScanlineSource<'a> {
    rgba: &'a [u8],
    row_bytes: usize,
    row: usize,
    offset: usize,
}

impl<'a> ScanlineSource<'a> {
    fn next_slice(&mut self, limit: usize) -> &'a [u8] {
        if self.offset == 0 {
            self.offset = 1;
            return &FILTER_NONE[..];
        }
        let consumed = self.offset - 1;
        let take = (self.row_bytes - consumed).min(limit);
        let start = self.row * self.row_bytes + consumed;
        let slice = &self.rgba[start..start + take];
        self.offset += take;
        if self.offset > self.row_bytes {
            self.offset = 0;
            self.row += 1;
        }
        slice
    }
}

struct Adler32 {
    low: u32,
    high: u32,
}

impl Adler32 {
    fn new() -> Adler32 {
        Adler32 { low: 1, high: 0 }
    }

    fn update(&mut self, data: &[u8]) {
        for batch in data.chunks(5552) {
            for &byte in batch {
                self.low += byte as u32;
                self.high += self.low;
            }
            self.low %= 65521;
            self.high %= 65521;
        }
    }

    fn value(&self) -> u32 {
        (self.high << 16) | self.low
    }
}

const fn build_crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut entry = 0usize;
    while entry < 256 {
        let mut value = entry as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 != 0 {
                0xEDB8_8320 ^ (value >> 1)
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[entry] = value;
        entry += 1;
    }
    table
}

fn update_crc(crc: u32, data: &[u8]) -> u32 {
    let mut value = crc;
    for &byte in data {
        value = CRC_TABLE[((value ^ byte as u32) & 0xFF) as usize] ^ (value >> 8);
    }
    value
}

fn write_vec3_array<W: Write>(out: &mut W, values: &[[f32; 3]]) -> io::Result<()> {
    let mut scratch = [0u8; 3072];
    for batch in values.chunks(256) {
        for (slot, value) in batch.iter().enumerate() {
            let base = slot * 12;
            scratch[base..base + 4].copy_from_slice(&sanitize(value[0]).to_le_bytes());
            scratch[base + 4..base + 8].copy_from_slice(&sanitize(value[1]).to_le_bytes());
            scratch[base + 8..base + 12].copy_from_slice(&sanitize(value[2]).to_le_bytes());
        }
        out.write_all(&scratch[..batch.len() * 12])?;
    }
    Ok(())
}

fn write_u32_array<W: Write>(out: &mut W, values: &[u32]) -> io::Result<()> {
    let mut scratch = [0u8; 1024];
    for batch in values.chunks(256) {
        for (slot, value) in batch.iter().enumerate() {
            let base = slot * 4;
            scratch[base..base + 4].copy_from_slice(&value.to_le_bytes());
        }
        out.write_all(&scratch[..batch.len() * 4])?;
    }
    Ok(())
}

fn push_json(json: &mut String, args: fmt::Arguments<'_>) {
    let _ = fmt::Write::write_fmt(json, args);
}

fn sanitize(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

fn srgb_to_linear(channel: u8) -> f32 {
    let value = channel as f32 / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}
