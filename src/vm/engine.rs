use super::*;

impl Env for Vm {
    fn get_var(&mut self, name: &str) -> Value {
        match self.lookup(name) {
            Some(v) => v.clone(),
            None => match self.system_var(name) {
                Some(v) => v,
                None => {
                    // LiveMaker trata las variables sin declarar como 0/""; solo se registra en debug
                    if self.warned.insert(format!("undef:{name}")) {
                        log::debug!("read of undefined variable {name} (as 0/\"\")");
                    }
                    Value::Empty
                }
            },
        }
    }
    fn set_var(&mut self, name: &str, v: Value) {
        self.assign(name, v);
    }
    fn call_engine(&mut self, f: u8, a: &[Value], raw: &[Param]) -> Option<Value> {
        let s = |i: usize| a.get(i).map(|v| v.as_str()).unwrap_or_default();
        Some(match f {
            Func::GETITEM => {
                let index = a
                    .get(1)
                    .map(Value::as_int)
                    .and_then(|i| usize::try_from(i).ok());
                Value::Str(
                    index
                        .and_then(|i| self.prevmenus.get(&s(0))?.buttons.get(i))
                        .map(|b| b.name.clone())
                        .unwrap_or_default(),
                )
            }
            Func::GETPROP => {
                let p = prop_of(a.get(1)?)?;
                if p == Prop::COUNT
                    && let Some(pm) = self.prevmenus.get(&s(0))
                {
                    return Some(Value::Int(pm.buttons.len() as i64));
                }
                self.get_prop(&s(0), p)
            }
            Func::SETPROP => {
                let p = prop_of(a.get(1)?)?;
                if s(0) == ROOT {
                    return Some(Value::Bool(self.scene.set(
                        ROOT,
                        p,
                        a.get(2).cloned().unwrap_or_default(),
                    )));
                }
                Value::Bool(
                    self.scene
                        .set(&s(0), p, a.get(2).cloned().unwrap_or_default()),
                )
            }
            Func::EXISTS => Value::Bool(self.scene.get(&s(0)).is_some()),
            Func::VAREXISTS => {
                let n = match raw.first() {
                    Some(Param::Var(n)) if !n.starts_with("____") => n.clone(),
                    _ => s(0),
                };
                Value::Bool(self.lookup(&n).is_some())
            }
            Func::GETVISIBLE => Value::Bool(if let Some(item) = a.get(1) {
                match item {
                    Value::Str(_) => self.menu_item_visible_key(&s(0), &menu_item_key(Some(item))),
                    _ => {
                        let Ok(index) = usize::try_from(item.as_int()) else {
                            return Some(Value::Bool(false));
                        };
                        self.menu_item_visible(&s(0), index)
                    }
                }
            } else {
                self.scene.get(&s(0)).is_some_and(|o| o.visible())
            }),
            Func::SETVISIBLE => {
                if let (Some(index), Some(value)) = (a.get(1), a.get(2)) {
                    if !matches!(index, Value::Str(_)) && usize::try_from(index.as_int()).is_err() {
                        return Some(Value::Bool(false));
                    }
                    self.visible
                        .insert((s(0), menu_item_key(Some(index))), value.truthy());
                    Value::Bool(true)
                } else {
                    Value::Bool(self.scene.set(
                        &s(0),
                        Prop::VISIBLE,
                        Value::Bool(a.get(1).is_some_and(|v| v.truthy())),
                    ))
                }
            }
            Func::INCABINET => Value::Bool(
                self.cabinets
                    .values()
                    .any(|objs| objs.iter().any(|o| o.name == s(0))),
            ),
            Func::FILEEXISTS => Value::Bool(self.assets.data().find(&s(0)).is_some()),
            Func::GETHISTORYCOUNT => Value::Int(self.history.len() as i64),
            Func::GETGROUPINDEX => {
                let (group, item_name) = self
                    .prevmenu_button(&s(0), a.get(1))
                    .map(|button| (button.group, button.name.clone()))
                    .unwrap_or((0, String::new()));
                if group != 0 {
                    return Some(Value::Int(i64::from(group)));
                }
                let object = if item_name.is_empty() {
                    self.scene.get(&s(0))
                } else {
                    self.scene.get(&item_name).or_else(|| self.scene.get(&s(0)))
                };
                Value::Int(
                    object
                        .map(|object| object.int(Prop::GROUPINDEX, 0))
                        .unwrap_or(0),
                )
            }
            // ListCompo(arr, modo, objeto): nombres de los componentes hijos de `objeto`
            // en orden de creación (modo != 0: también descendientes; semántica inferida).
            Func::LISTCOMPO => {
                let list = self
                    .scene
                    .children(&s(2), a.get(1).map(|v| v.as_int() != 0).unwrap_or(false));
                let n = list.len() as i64;
                if let Some(Param::Var(var)) = raw.first() {
                    let var = var.clone();
                    self.assign(
                        &var,
                        Value::Array(list.into_iter().map(Value::Str).collect()),
                    );
                }
                Value::Int(n)
            }
            // Ítems de PrevMenu/menú: (objeto, índice[, valor]). Semántica inferida.
            Func::SETENABLED | Func::SETSELECTED => {
                let k = (s(0), menu_item_key(a.get(1)));
                let v = a.get(2).map(|v| v.truthy()).unwrap_or(true);
                if f == Func::SETENABLED {
                    self.enabled.insert(k, v)
                } else {
                    self.selected.insert(k, v)
                };
                Value::Bool(true)
            }
            Func::GETENABLED => Value::Bool(
                *self
                    .enabled
                    .get(&(s(0), menu_item_key(a.get(1))))
                    .unwrap_or(&true),
            ),
            Func::ADDDELIMITER => {
                let has_content = match a.get(1) {
                    None | Some(Value::Empty) => false,
                    Some(Value::Array(values)) => !values.is_empty(),
                    Some(value) => !value.as_str().is_empty(),
                };
                Value::Str(if has_content { s(0) } else { String::new() })
            }
            Func::GETSELECTED => Value::Bool(
                *self
                    .selected
                    .get(&(s(0), menu_item_key(a.get(1))))
                    .unwrap_or(&false),
            ),
            // Menú: checkmark por índice/nombre (manual LiveNovel GetCheck/SetCheck).
            Func::SETCHECK => {
                let k = (s(0), menu_item_key(a.get(1)));
                let v = a.get(2).map(|v| v.truthy()).unwrap_or(true);
                self.checked.insert(k, v);
                Value::Bool(true)
            }
            Func::GETCHECK => Value::Bool(
                *self
                    .checked
                    .get(&(s(0), menu_item_key(a.get(1))))
                    .unwrap_or(&false),
            ),
            // InPrimary: no documentada; se asume "ventana principal activa" -> verdadero.
            Func::INPRIMARY => Value::Bool(true),
            Func::GETHISTORYMAXCOUNT => Value::Int(self.history_max),
            Func::SETHISTORYMAXCOUNT => {
                self.history_max = a.first().map(|v| v.as_int()).unwrap_or(0).max(0);
                if self.history_max > 0 {
                    while self.history.len() > self.history_max as usize {
                        self.history.pop_front();
                    }
                }
                Value::Bool(true)
            }
            // Ventana = espacio lógico (letterbox lo resuelve SDL), así que la conversión es identidad
            Func::TOSCREENX | Func::TOSCREENY | Func::TOCLIENTX | Func::TOCLIENTY => {
                a.last().cloned().unwrap_or(Value::Int(0))
            }
            // docs/livenovel/calc.html: LoadTextFile(variable, archivo) deja en la
            // variable un array de una entrada por línea y devuelve verdadero si leyó.
            Func::LOADTEXTFILE => {
                let Some(var) = (match raw.first() {
                    Some(Param::Var(name)) if !name.starts_with("____") => Some(name.clone()),
                    Some(Param::Str(name)) if !name.is_empty() => Some(name.clone()),
                    _ => None,
                }) else {
                    return Some(Value::Bool(false));
                };
                let path = a.get(1).map(Value::as_str).unwrap_or_default();
                let data = self.assets.data().clone();
                let Some(name) = data.resolve(&path, &[".txt"]) else {
                    return Some(Value::Bool(false));
                };
                let started = std::time::Instant::now();
                let Some(bytes) = data.read(&name).ok() else {
                    return Some(Value::Bool(false));
                };
                let text = crate::util::cp932(&bytes);
                let lines: Vec<Value> = text
                    .lines()
                    .map(|line| Value::Str(line.to_string()))
                    .collect();
                log::info!(
                    "load text {name}: {} bytes, {} lines in {:.1} ms",
                    bytes.len(),
                    lines.len(),
                    started.elapsed().as_secs_f64() * 1000.0
                );
                self.assign(&var, Value::Array(lines));
                Value::Bool(true)
            }
            Func::DBCREATETABLE..=Func::DBGETRECNO
            | Func::DBCLEARRECORD
            | Func::DBGETTABLECOUNT
            | Func::DBGETTABLE => {
                let d = self.assets.data().clone();
                let load = move |f: &str| {
                    let name = d.resolve(f, &[".tsv", ".txt"])?;
                    let started = std::time::Instant::now();
                    let bytes = d.read(&name).ok()?;
                    log::info!(
                        "load table {name}: {} bytes in {:.1} ms",
                        bytes.len(),
                        started.elapsed().as_secs_f64() * 1000.0
                    );
                    Some(crate::util::cp932(&bytes))
                };
                return self.db.call(f, a, &load);
            }
            // **interpretación:** siempre modo ventana lógica (0); no hay exclusive fullscreen LM.
            Func::GETDISPLAYMODE => Value::Int(0),
            Func::GETSAVECAPTION => {
                let slot = a.first().map(|v| v.as_int()).unwrap_or(-1);
                let caption = crate::save::read_save_caption(&self.save_dir, slot);
                Value::Str(caption)
            }
            // script.html `<OPENURL>` / calc OPENURL: abrir en el navegador del SO.
            Func::OPENURL => {
                let url = a.first().map(Value::as_str).unwrap_or_default();
                Value::Bool(open_external_url(&url))
            }
            // calc.html SetWallPaper: wallpaper del SO. Sin API host → FALSE honesto.
            Func::SETWALLPAPER => {
                self.warn_once(
                    "wallpaper:unsupported",
                    "SetWallPaper: desktop wallpaper not supported (returns false)",
                );
                Value::Bool(false)
            }
            // calc.html SaveScreen: captura pantalla → .gal / mem: / save:.
            Func::SAVESCREEN => Value::Bool(self.save_screen(a)),
            // calc.html CopyFile: mem:/save:/host; sobrescribe destino.
            Func::COPYFILE => {
                let src = a.first().map(Value::as_str).unwrap_or_default();
                let dst = a.get(1).map(Value::as_str).unwrap_or_default();
                Value::Bool(copy_file(self, &src, &dst))
            }
            _ => return None,
        })
    }

    fn random(&mut self) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 11) as f64 / (1u64 << 53) as f64
    }
    fn warn_once(&mut self, key: &str, msg: &str) {
        let k = if msg.starts_with("unimplemented function") {
            format!("unimpl:func {key}")
        } else {
            key.to_string()
        };
        self.warn(&k, msg);
    }
}

/// Prefijo `mem:` / `save:` sin distinguir mayúsculas (calc.html).
pub(super) fn strip_vfs_prefix<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    if path.len() >= prefix.len()
        && path.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
    {
        Some(&path[prefix.len()..])
    } else {
        None
    }
}

pub(super) fn read_vfs_bytes(vm: &Vm, path: &str) -> Option<Vec<u8>> {
    if let Some(name) = strip_vfs_prefix(path, "mem:") {
        return vm
            .mem_files
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone());
    }
    if let Some(name) = strip_vfs_prefix(path, "save:") {
        return crate::save::read_save_blob(&vm.save_dir, name);
    }
    let src_path = if PathBuf::from(path).is_absolute() {
        PathBuf::from(path)
    } else {
        vm.save_dir.join(path.replace('\\', "/"))
    };
    std::fs::read(&src_path).ok()
}

pub(super) fn resample_nearest(rgba: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let mut out = vec![0u8; (dw as usize) * (dh as usize) * 4];
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return out;
    }
    for y in 0..dh as usize {
        let sy = y * sh as usize / dh as usize;
        for x in 0..dw as usize {
            let sx = x * sw as usize / dw as usize;
            let si = (sy * sw as usize + sx) * 4;
            let di = (y * dw as usize + x) * 4;
            if si + 4 <= rgba.len() {
                out[di..di + 4].copy_from_slice(&rgba[si..si + 4]);
            }
        }
    }
    out
}

pub(super) fn write_vfs_bytes(vm: &mut Vm, path: &str, data: &[u8]) -> bool {
    if let Some(name) = strip_vfs_prefix(path, "mem:") {
        if name.is_empty() {
            return false;
        }
        vm.mem_files.retain(|k, _| !k.eq_ignore_ascii_case(name));
        vm.mem_files.insert(name.to_string(), data.to_vec());
        vm.vfs_image_cache
            .borrow_mut()
            .retain(|k, _| strip_vfs_prefix(k, "mem:") != Some(name));
        return true;
    }
    if let Some(name) = strip_vfs_prefix(path, "save:") {
        if name.is_empty() {
            return false;
        }
        let ok = crate::save::write_save_blob(&vm.save_dir, name, data).is_ok();
        if ok {
            vm.vfs_image_cache
                .borrow_mut()
                .retain(|k, _| strip_vfs_prefix(k, "save:") != Some(name));
        }
        return ok;
    }
    let dst_path = if PathBuf::from(path).is_absolute() {
        PathBuf::from(path)
    } else {
        vm.save_dir.join(path.replace('\\', "/"))
    };
    if let Some(parent) = dst_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&dst_path, data) {
        Ok(()) => true,
        Err(e) => {
            log::warn!("CopyFile write {dst_path:?}: {e}");
            false
        }
    }
}

/// CopyFile (calc.html): copia mem/save/archivos de host; sobrescribe destino.
fn copy_file(vm: &mut Vm, src: &str, dst: &str) -> bool {
    if src.is_empty() || dst.is_empty() {
        return false;
    }
    let Some(data) = read_vfs_bytes(vm, src) else {
        log::warn!("CopyFile: source not found {src:?}");
        return false;
    };
    write_vfs_bytes(vm, dst, &data)
}

/// Abre una URL con el manejador del sistema. **fuente primaria** script.html OPENURL.
pub(crate) fn open_external_url(url: &str) -> bool {
    let url = url.trim();
    if url.is_empty() {
        return false;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
            .is_ok()
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn().is_ok()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .is_ok()
    }
}
