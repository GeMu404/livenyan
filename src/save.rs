//! Manejo de partidas guardadas compatibles con el formato original de LiveMaker (`save.dat`).
//!
//! LiveMaker almacena tanto las opciones globales (`STATUSSAVEDATA`) como los slots
//! de juego (`GAMESAVEDATA0` .. `GAMESAVEDATA29`) dentro de un único archivo VF (`save.dat`).

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::archive::{decompress, parse_directory, write_vf_archive};
use crate::lsb::expr::Value;
use crate::util::{cp932, to_cp932};

const SAVE_MAGIC: &[u8] = b"NySave103\x00";

/// Entrada deserializada de un slot de guardado.
#[derive(Debug, Clone)]
pub struct SaveSlot {
    pub caption: String,
    pub page: String,
    pub label: u32,
    pub globals: HashMap<String, Value>,
}

/// Ruta canónica a `save.dat` (busca en el directorio del juego o en data_dir).
pub fn save_file_path(game_dir: &Path) -> PathBuf {
    game_dir.join("save.dat")
}

/// Lee todas las entradas de `save.dat` si existe.
pub fn read_all_save_entries(path: &Path) -> io::Result<Vec<(String, Vec<u8>)>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let data = fs::read(path)?;
    if data.len() < 2 || &data[..2] != b"vf" {
        return Ok(Vec::new());
    }
    let (_, entries) = parse_directory(&data)?;
    let mut result = Vec::with_capacity(entries.len());
    for entry in entries {
        let end = (entry.offset + entry.size) as usize;
        if end <= data.len() {
            let slice = &data[entry.offset as usize..end];
            if let Ok(decomp) = decompress(&entry, slice.to_vec()) {
                result.push((entry.name, decomp));
            }
        }
    }
    Ok(result)
}

/// Serializa las variables en formato binario compatible con LiveMaker Status103.
pub fn encode_status_variables(vars: &HashMap<String, Value>) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"Status103");
    out.extend_from_slice(&(vars.len() as u32).to_le_bytes());

    for (name, val) in vars {
        let name_bytes = to_cp932(name);
        match val {
            Value::Bool(b) => {
                out.push(3u8); // Type 3 = Bool
                out.extend_from_slice(&(name_bytes.len() as u32).to_le_bytes());
                out.extend_from_slice(&name_bytes);
                out.extend_from_slice(&0u32.to_le_bytes()); // scope
                out.extend_from_slice(&1u32.to_le_bytes()); // arr_len
                out.push(if *b { 1u8 } else { 0u8 });
                // Extra padding flags
                out.extend_from_slice(&[1, 0, 0xff, 0xff, 0xff, 0xff, 0]);
            }
            Value::Int(i) => {
                out.push(1u8); // Type 1 = Int
                out.extend_from_slice(&(name_bytes.len() as u32).to_le_bytes());
                out.extend_from_slice(&name_bytes);
                out.extend_from_slice(&0u32.to_le_bytes()); // scope
                out.extend_from_slice(&1u32.to_le_bytes()); // arr_len
                out.extend_from_slice(&(*i as i32).to_le_bytes());
                out.extend_from_slice(&[1, 0, 0xff, 0xff, 0xff, 0xff, 0]);
            }
            Value::Float(f) => {
                out.push(2u8); // Type 2 = Float
                out.extend_from_slice(&(name_bytes.len() as u32).to_le_bytes());
                out.extend_from_slice(&name_bytes);
                out.extend_from_slice(&0u32.to_le_bytes()); // scope
                out.extend_from_slice(&1u32.to_le_bytes()); // arr_len
                out.extend_from_slice(&f.to_le_bytes());
                out.extend_from_slice(&[1, 0, 0xff, 0xff, 0xff, 0xff, 0]);
            }
            Value::Str(s) => {
                out.push(4u8); // Type 4 = Str
                out.extend_from_slice(&(name_bytes.len() as u32).to_le_bytes());
                out.extend_from_slice(&name_bytes);
                out.extend_from_slice(&0u32.to_le_bytes()); // scope
                out.extend_from_slice(&1u32.to_le_bytes()); // arr_len
                let s_bytes = to_cp932(s);
                out.extend_from_slice(&(s_bytes.len() as u32).to_le_bytes());
                out.extend_from_slice(&s_bytes);
                out.extend_from_slice(&[1, 0, 0xff, 0xff, 0xff, 0xff, 0]);
            }
            _ => {}
        }
    }
    out
}

/// Decodifica variables en formato binario Status103.
pub fn decode_status_variables(data: &[u8]) -> io::Result<HashMap<String, Value>> {
    let mut vars = HashMap::new();
    if data.len() < 13 || !data.starts_with(b"Status103") {
        return Ok(vars);
    }
    let mut pos = 9;
    let count = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
    pos += 4;

    for _ in 0..count {
        if pos >= data.len() {
            break;
        }
        let vtype = data[pos];
        pos += 1;
        if pos + 4 > data.len() {
            break;
        }
        let namelen = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        if pos + namelen > data.len() {
            break;
        }
        let name = cp932(&data[pos..pos + namelen]);
        pos += namelen;

        // Skip scope y arr_len (8 bytes)
        pos += 8;
        if pos > data.len() {
            break;
        }

        let val = match vtype {
            3 => {
                if pos >= data.len() {
                    break;
                }
                let b = data[pos] != 0;
                pos += 1;
                Value::Bool(b)
            }
            1 => {
                if pos + 4 > data.len() {
                    break;
                }
                let i = i32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
                pos += 4;
                Value::Int(i as i64)
            }
            2 => {
                if pos + 8 > data.len() {
                    break;
                }
                let f = f64::from_le_bytes(data[pos..pos + 8].try_into().unwrap());
                pos += 8;
                Value::Float(f)
            }
            4 => {
                // Str (u32 len + bytes)
                if pos + 4 > data.len() {
                    break;
                }
                let slen = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
                pos += 4;
                if pos + slen > data.len() {
                    break;
                }
                let s = cp932(&data[pos..pos + slen]);
                pos += slen;
                Value::Str(s)
            }
            _ => Value::Empty,
        };

        vars.insert(name, val);
        // Saltamos los 7 bytes de extra flags
        pos += 7;
    }

    Ok(vars)
}

/// Codifica el contenido completo de una entrada `GAMESAVEDATA<slot>`.
pub fn encode_game_save_slot(slot: &SaveSlot) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(SAVE_MAGIC);

    // Caption en UTF-8 para soportar tildes, japonés y caracteres de cualquier idioma
    let cap_bytes = slot.caption.as_bytes();
    out.extend_from_slice(&(cap_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(cap_bytes);

    let page_bytes = slot.page.as_bytes();
    out.extend_from_slice(&(page_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(page_bytes);

    out.extend_from_slice(&slot.label.to_le_bytes());

    let vars_bytes = encode_status_variables(&slot.globals);
    out.extend_from_slice(&(vars_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&vars_bytes);

    out
}

/// Decodifica una entrada `GAMESAVEDATA<slot>`.
pub fn decode_game_save_slot(data: &[u8]) -> io::Result<SaveSlot> {
    if data.starts_with(SAVE_MAGIC) {
        let mut pos = SAVE_MAGIC.len();
        if pos + 4 > data.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "save slot truncated",
            ));
        }
        let cap_len = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        let caption = String::from_utf8_lossy(&data[pos..pos + cap_len]).into_owned();
        pos += cap_len;

        let page_len = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        let page = String::from_utf8_lossy(&data[pos..pos + page_len]).into_owned();
        pos += page_len;

        let label = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
        pos += 4;

        let vars_len = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        let globals = decode_status_variables(&data[pos..pos + vars_len])?;

        Ok(SaveSlot {
            caption,
            page,
            label,
            globals,
        })
    } else {
        // Compatibilidad con save original de LiveMaker si empieza con Status103 o estructura clásica
        let globals = decode_status_variables(data)?;
        Ok(SaveSlot {
            caption: String::new(),
            page: String::new(),
            label: 0,
            globals,
        })
    }
}

/// Consulta el caption de un slot de guardado sin necesidad de deserializar todo el estado.
pub fn read_save_caption(game_dir: &Path, slot: i64) -> String {
    if slot < 0 {
        return String::new();
    }
    let save_path = save_file_path(game_dir);
    let Ok(entries) = read_all_save_entries(&save_path) else {
        return String::new();
    };

    let target_name = format!("GAMESAVEDATA{slot}");
    for (name, data) in entries {
        if name.eq_ignore_ascii_case(&target_name)
            && let Ok(slot_data) = decode_game_save_slot(&data)
        {
            return slot_data.caption;
        }
    }
    String::new()
}

/// Guarda una partida en el archivo `save.dat` bajo `GAMESAVEDATA{slot}`.
pub fn save_game(
    game_dir: &Path,
    slot: i64,
    page: &str,
    label: u32,
    caption: &str,
    globals: &HashMap<String, Value>,
) -> io::Result<()> {
    let save_path = save_file_path(game_dir);
    let mut all_entries = read_all_save_entries(&save_path).unwrap_or_default();

    let slot_entry = SaveSlot {
        caption: caption.to_string(),
        page: page.to_string(),
        label,
        globals: globals.clone(),
    };
    let encoded = encode_game_save_slot(&slot_entry);

    let target_name = format!("GAMESAVEDATA{slot}");
    let mut replaced = false;
    for (name, content) in &mut all_entries {
        if name.eq_ignore_ascii_case(&target_name) {
            *content = encoded.clone();
            replaced = true;
            break;
        }
    }
    if !replaced {
        all_entries.push((target_name, encoded));
    }

    // Asegurarse de que STATUSSAVEDATA también se mantenga o cree
    let has_status = all_entries.iter().any(|(n, _)| n == "STATUSSAVEDATA");
    if !has_status {
        let status_bytes = encode_status_variables(globals);
        all_entries.insert(0, ("STATUSSAVEDATA".to_string(), status_bytes));
    }

    let refs: Vec<(&str, &[u8])> = all_entries
        .iter()
        .map(|(n, c)| (n.as_str(), c.as_slice()))
        .collect();

    let vf_bytes = write_vf_archive(&refs)?;

    // Escritura atómica vía archivo temporal
    let tmp_path = game_dir.join("save.dat.$$$");
    fs::write(&tmp_path, &vf_bytes)?;
    fs::rename(&tmp_path, &save_path)?;

    log::info!("Guardada partida en slot {slot} en {}", save_path.display());
    Ok(())
}

/// Lee un blob con nombre propio dentro de `save.dat` (rutas `save:…` de calc.html).
pub fn read_save_blob(game_dir: &Path, name: &str) -> Option<Vec<u8>> {
    let save_path = save_file_path(game_dir);
    let Ok(entries) = read_all_save_entries(&save_path) else {
        return None;
    };
    entries
        .into_iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, data)| data)
}

/// Escribe/sobrescribe un blob con nombre propio en `save.dat` (`save:…`).
pub fn write_save_blob(game_dir: &Path, name: &str, data: &[u8]) -> io::Result<()> {
    let save_path = save_file_path(game_dir);
    let mut all_entries = read_all_save_entries(&save_path).unwrap_or_default();
    let mut replaced = false;
    for (entry_name, content) in &mut all_entries {
        if entry_name.eq_ignore_ascii_case(name) {
            *content = data.to_vec();
            replaced = true;
            break;
        }
    }
    if !replaced {
        all_entries.push((name.to_string(), data.to_vec()));
    }
    let refs: Vec<(&str, &[u8])> = all_entries
        .iter()
        .map(|(n, c)| (n.as_str(), c.as_slice()))
        .collect();
    let vf_bytes = write_vf_archive(&refs)?;
    let tmp_path = game_dir.join("save.dat.$$$");
    fs::write(&tmp_path, &vf_bytes)?;
    fs::rename(&tmp_path, &save_path)?;
    Ok(())
}

/// Carga una partida desde el archivo `save.dat`.
pub fn load_game(game_dir: &Path, slot: i64) -> io::Result<SaveSlot> {
    let save_path = save_file_path(game_dir);
    let all_entries = read_all_save_entries(&save_path)?;
    let target_name = format!("GAMESAVEDATA{slot}");

    for (name, data) in all_entries {
        if name.eq_ignore_ascii_case(&target_name) {
            return decode_game_save_slot(&data);
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("Slot {slot} no encontrado en {}", save_path.display()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_save_slot_roundtrip() {
        let mut globals = HashMap::new();
        globals.insert("Score".to_string(), Value::Int(9999));
        globals.insert("PlayerName".to_string(), Value::Str("NyanHero".to_string()));
        globals.insert("FlagClear".to_string(), Value::Bool(true));

        let slot = SaveSlot {
            caption: "Capítulo 1 - Inicio".to_string(),
            page: "ノベルシステム\\セーブ.lsb".to_string(),
            label: 46,
            globals: globals.clone(),
        };

        let encoded = encode_game_save_slot(&slot);
        let decoded = decode_game_save_slot(&encoded).expect("decode save slot ok");

        assert_eq!(decoded.caption, "Capítulo 1 - Inicio");
        assert_eq!(decoded.page, "ノベルシステム\\セーブ.lsb");
        assert_eq!(decoded.label, 46);
        assert_eq!(decoded.globals.get("Score"), Some(&Value::Int(9999)));
        assert_eq!(
            decoded.globals.get("PlayerName"),
            Some(&Value::Str("NyanHero".to_string()))
        );
        assert_eq!(decoded.globals.get("FlagClear"), Some(&Value::Bool(true)));
    }

    #[test]
    fn test_save_dat_file_lifecycle() {
        let temp_dir = std::env::temp_dir().join("livenyan_save_test");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).expect("create temp dir ok");

        let mut globals = HashMap::new();
        globals.insert("StatusBGMVolume".to_string(), Value::Int(800));
        globals.insert("SceneIndex".to_string(), Value::Int(42));

        save_game(&temp_dir, 0, "main.lsb", 10, "Partida 001", &globals).expect("save 0 ok");

        assert_eq!(read_save_caption(&temp_dir, 0), "Partida 001");
        assert_eq!(read_save_caption(&temp_dir, 1), ""); // slot 1 aun vacio

        globals.insert("SceneIndex".to_string(), Value::Int(99));
        save_game(&temp_dir, 1, "next.lsb", 25, "Partida 002 Final", &globals).expect("save 1 ok");

        assert_eq!(read_save_caption(&temp_dir, 0), "Partida 001");
        assert_eq!(read_save_caption(&temp_dir, 1), "Partida 002 Final");

        let loaded0 = load_game(&temp_dir, 0).expect("load 0 ok");
        assert_eq!(loaded0.page, "main.lsb");
        assert_eq!(loaded0.label, 10);
        assert_eq!(loaded0.globals.get("SceneIndex"), Some(&Value::Int(42)));

        let loaded1 = load_game(&temp_dir, 1).expect("load 1 ok");
        assert_eq!(loaded1.page, "next.lsb");
        assert_eq!(loaded1.label, 25);
        assert_eq!(loaded1.globals.get("SceneIndex"), Some(&Value::Int(99)));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn save_blob_roundtrip_in_save_dat() {
        let temp_dir = std::env::temp_dir().join("livenyan_save_blob_test");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).expect("create temp dir ok");
        write_save_blob(&temp_dir, "__savescr0.gal", b"gal-bytes").expect("write blob");
        assert_eq!(
            read_save_blob(&temp_dir, "__savescr0.gal").as_deref(),
            Some(b"gal-bytes".as_slice())
        );
        // Convive con un slot de partida.
        let mut globals = HashMap::new();
        globals.insert("x".into(), Value::Int(1));
        save_game(&temp_dir, 0, "p.lsb", 1, "c", &globals).expect("save game");
        assert_eq!(
            read_save_blob(&temp_dir, "__savescr0.gal").as_deref(),
            Some(b"gal-bytes".as_slice()),
            "GameSave must not drop named save: blobs"
        );
        let _ = fs::remove_dir_all(&temp_dir);
    }
}
