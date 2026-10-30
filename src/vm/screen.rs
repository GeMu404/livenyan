use super::*;

impl Vm {
    /// Imagen desde `mem:` / `save:` si el SOURCE no está en el archivo del juego.
    pub fn vfs_image(&self, src: &str) -> Option<std::sync::Arc<crate::gal::Image>> {
        if strip_vfs_prefix(src, "mem:").is_none() && strip_vfs_prefix(src, "save:").is_none() {
            return None;
        }
        if let Some(img) = self.vfs_image_cache.borrow().get(src) {
            return Some(img.clone());
        }
        let bytes = read_vfs_bytes(self, src)?;
        let img = std::sync::Arc::new(crate::gal::decode_any(&bytes).ok()?);
        self.vfs_image_cache
            .borrow_mut()
            .insert(src.to_string(), img.clone());
        Some(img)
    }

    /// ¿Hay un SaveScreen esperando readback de la escena?
    pub fn take_pending_savescreen(&mut self) -> Option<SaveScreenRequest> {
        self.pending_savescreen.take()
    }

    /// Completa un SaveScreen diferido con RGBA recién leído del target de escena.
    pub fn complete_savescreen(
        &mut self,
        req: &SaveScreenRequest,
        w: u32,
        h: u32,
        rgba: Vec<u8>,
    ) -> bool {
        if w == 0 || h == 0 || rgba.len() < (w as usize) * (h as usize) * 4 {
            return false;
        }
        self.screen_rgba = Some((w, h, rgba));
        self.apply_savescreen(req)
    }

    /// Solo tests / inyección: deja un frame en CPU sin pasar por SDL.
    #[cfg(test)]
    pub fn set_screen_rgba(&mut self, w: u32, h: u32, rgba: Vec<u8>) {
        if w == 0 || h == 0 || rgba.len() < (w as usize) * (h as usize) * 4 {
            return;
        }
        self.screen_rgba = Some((w, h, rgba));
    }

    /// calc.html SaveScreen. Si ya hay captura CPU, escribe ya; si no, encola readback
    /// una sola vez tras el render (**interpretación**: TRUE = se cumplirá este frame).
    pub(super) fn save_screen(&mut self, a: &[Value]) -> bool {
        let path = a.first().map(Value::as_str).unwrap_or_default();
        if path.is_empty() {
            return false;
        }
        let rw = a.get(1).map(|v| v.as_int()).unwrap_or(0).max(0) as u32;
        let rh = a.get(2).map(|v| v.as_int()).unwrap_or(0).max(0) as u32;
        // ④ back buffer / ⑤ delete: un solo buffer lógico (**no confirmado** / omitido).
        let req = SaveScreenRequest {
            path,
            resample_w: rw,
            resample_h: rh,
        };
        if self.screen_rgba.is_some() {
            return self.apply_savescreen(&req);
        }
        self.pending_savescreen = Some(req);
        true
    }

    fn apply_savescreen(&mut self, req: &SaveScreenRequest) -> bool {
        let Some((sw, sh, rgba)) = self.screen_rgba.as_ref() else {
            return false;
        };
        let (w, h, pixels) = if req.resample_w > 0 && req.resample_h > 0 {
            (
                req.resample_w,
                req.resample_h,
                resample_nearest(rgba, *sw, *sh, req.resample_w, req.resample_h),
            )
        } else {
            (*sw, *sh, rgba.clone())
        };
        // Encode zlib es barato frente al readback; se puede mover a worker después.
        let gal = match crate::gal::encode_galex_rgba(w, h, &pixels) {
            Ok(g) => g,
            Err(e) => {
                log::warn!("SaveScreen encode: {e}");
                return false;
            }
        };
        let ok = write_vfs_bytes(self, &req.path, &gal);
        if ok {
            self.vfs_image_cache.borrow_mut().remove(&req.path);
        }
        ok
    }

    fn wipe_set_has(scene: &Scene, set: &HashSet<String>, name: &str) -> bool {
        let mut current = name.to_string();
        for _ in 0..64 {
            if set.contains(&current) {
                return true;
            }
            let Some(parent) = scene
                .get(&current)
                .map(|object| object.str(Prop::PARENT))
                .filter(|parent| !parent.is_empty())
            else {
                return false;
            };
            current = parent;
        }
        false
    }

    /// RGBA del overlay Flip WHITE/BLACK/FLASH, si activo.
    pub fn wipe_flash_rgba(&self) -> Option<(u8, u8, u8, u8)> {
        self.wipe_flash.as_ref().and_then(|f| f.rgba(self.now))
    }

    /// Factor 0–1 de la superficie de este objeto durante un Flip, si le toca.
    pub fn wipe_alpha_factor(&self, name: &str) -> Option<f64> {
        let fade = self.wipe_fade.as_ref()?;
        let progress = fade.progress(self.now);
        if Self::wipe_set_has(&self.scene, &fade.out, name) {
            Some(1.0 - progress)
        } else if Self::wipe_set_has(&self.scene, &fade.inn, name) {
            Some(progress)
        } else {
            None
        }
    }

    /// Objetos de la superficie nueva que el orden normal no dibuja (están archivados).
    pub fn append_wipe_incoming(&self, ordered: &mut Vec<crate::scene::Obj>) {
        let Some(fade) = &self.wipe_fade else {
            return;
        };
        if fade.inn.is_empty() {
            return;
        }
        let have: HashSet<String> = ordered.iter().map(|object| object.name.clone()).collect();
        let mut extra: Vec<_> = self
            .scene
            .live()
            .filter(|object| {
                !have.contains(&object.name)
                    && Self::wipe_set_has(&self.scene, &fade.inn, &object.name)
            })
            .cloned()
            .collect();
        extra.sort_by_key(|object| (object.int(Prop::PRIORITY, 0), object.seq));
        ordered.extend(extra);
    }

    /// Offset de dibujo por `<QUAKE>` (0,0 si no aplica).
    pub fn quake_draw_offset(&self, name: &str) -> (f64, f64) {
        let Some(q) = self.quake.as_ref() else {
            return (0.0, 0.0);
        };
        if self.now >= q.until {
            return (0.0, 0.0);
        }
        if !q.names.is_empty() && !q.names.iter().any(|n| n == name) {
            return (0.0, 0.0);
        }
        let t = (self.now - q.started_at).max(0.0);
        let amp_scale = if let Some(stop_at) = q.stop_until {
            if self.now >= stop_at {
                return (0.0, 0.0);
            }
            // STOPQUAKE: amortiguar en TIME ms (**inventado**: escala lineal al deadline).
            ((stop_at - self.now) / (stop_at - q.started_at).max(1.0)).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let cycle = q.cycle_ms.max(1.0);
        let phase = if q.random_phase {
            // desfase estable por nombre
            let h = name
                .bytes()
                .fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
            t + f64::from(h % 1000)
        } else {
            t
        };
        let (ax, ay) = (q.amp_x * amp_scale, q.amp_y * amp_scale);
        let (dx, dy) = match q.kind {
            'W' => {
                let w = std::f64::consts::TAU * (phase / cycle);
                (ax * w.sin(), ay * w.sin())
            }
            'B' => {
                // rebote: abs(sin)
                let w = std::f64::consts::TAU * (phase / cycle);
                (0.0, ay * w.sin().abs())
            }
            _ => {
                // Q: pseudo-aleatorio acotado (**inventado**: hash del tiempo)
                let n = ((phase * 12.9898).sin() * 43758.5453).fract();
                let m = ((phase * 78.233).sin() * 43758.5453).fract();
                (ax * (n * 2.0 - 1.0), ay * (m * 2.0 - 1.0))
            }
        };
        (dx, dy)
    }
}
