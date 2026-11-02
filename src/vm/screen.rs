use super::*;

impl Vm {
    pub(crate) fn hold_wipe_until_scene_ready(&mut self) {
        if self.wipe_fade.is_some() {
            self.wipe_hold_started_at.get_or_insert(self.now);
        }
    }

    pub fn is_vfs_image_source(src: &str) -> bool {
        strip_vfs_prefix(src, "mem:").is_some() || strip_vfs_prefix(src, "save:").is_some()
    }

    /// Imagen desde `mem:` / `save:` si el SOURCE no está en el archivo del juego.
    pub fn vfs_image(&self, src: &str) -> Option<std::sync::Arc<crate::gal::Image>> {
        if !Self::is_vfs_image_source(src) {
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

    /// Pedidos de SaveScreen que esperan readback de la escena.
    pub fn take_pending_savescreen(&mut self) -> Vec<SaveScreenRequest> {
        std::mem::take(&mut self.pending_savescreen)
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
        self.apply_savescreen(req, w, h, &rgba)
    }

    /// calc.html SaveScreen. Pide un readback nuevo del último target presentado.
    /// Nunca reutiliza una captura CPU anterior.
    pub(super) fn save_screen(&mut self, a: &[Value]) -> bool {
        let path = a.first().map(Value::as_str).unwrap_or_default();
        if path.is_empty() {
            return false;
        }
        let rw = a.get(1).map(|v| v.as_int()).unwrap_or(0).max(0) as u32;
        let rh = a.get(2).map(|v| v.as_int()).unwrap_or(0).max(0) as u32;
        if let Some(name) = strip_vfs_prefix(&path, "mem:") {
            self.mem_files
                .retain(|key, _| !key.eq_ignore_ascii_case(name));
            self.vfs_image_cache.borrow_mut().retain(|cached, _| {
                strip_vfs_prefix(cached, "mem:")
                    .is_none_or(|cached_name| !cached_name.eq_ignore_ascii_case(name))
            });
        }
        let layout = if rw > 0 && rh > 0 {
            (rw, rh)
        } else {
            self.opts.screen
        };
        self.vfs_layout.insert(path.clone(), layout);
        self.pending_savescreen.push(SaveScreenRequest {
            path,
            resample_w: rw,
            resample_h: rh,
        });
        true
    }

    /// Layout lógico para que una textura SaveScreen a tamaño del target siga ocupando
    /// las coordenadas de presentación del objeto.
    pub fn savescreen_layout(&self, src: &str) -> Option<(u32, u32)> {
        self.vfs_layout.get(src).copied().or_else(|| {
            self.vfs_layout
                .iter()
                .find(|(path, _)| path.eq_ignore_ascii_case(src))
                .map(|(_, size)| *size)
        })
    }

    fn apply_savescreen(&mut self, req: &SaveScreenRequest, sw: u32, sh: u32, rgba: &[u8]) -> bool {
        let (w, h, pixels) = if req.resample_w > 0 && req.resample_h > 0 {
            (
                req.resample_w,
                req.resample_h,
                resample_nearest(rgba, sw, sh, req.resample_w, req.resample_h),
            )
        } else {
            (sw, sh, rgba.to_vec())
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
            self.vfs_image_cache
                .borrow_mut()
                .retain(|cached, _| !cached.eq_ignore_ascii_case(&req.path));
            let layout = if req.resample_w > 0 && req.resample_h > 0 {
                (req.resample_w, req.resample_h)
            } else {
                self.opts.screen
            };
            self.vfs_layout.insert(req.path.clone(), layout);
            if self.wipe_fade.is_some() {
                self.wipe_sync_capture = true;
            }
        }
        ok
    }

    /// El siguiente frame inicia el wipe con la captura ya escrita.
    pub(super) fn rewind_active_wipe(&mut self) {
        if let Some(fade) = self.wipe_fade.as_mut() {
            fade.started_at = self.now;
        }
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
        if matches!(fade.kind, wipe::WipeKind::Blind { .. }) {
            return None;
        }
        let progress = fade.progress(self.now);
        if Self::wipe_set_has(&self.scene, &fade.out, name) {
            Some(1.0 - progress)
        } else if Self::wipe_set_has(&self.scene, &fade.inn, name) {
            Some(progress)
        } else {
            None
        }
    }

    pub(crate) fn wipe_blind_strips(
        &self,
        name: &str,
        screen_w: u32,
        screen_h: u32,
    ) -> Option<Vec<wipe::Strip>> {
        let fade = self.wipe_fade.as_ref()?;
        let wipe::WipeKind::Blind {
            horizontal,
            span,
            appear_ms,
            reverse,
        } = fade.kind
        else {
            return None;
        };
        let incoming = Self::wipe_set_has(&self.scene, &fade.inn, name);
        let outgoing = !incoming && Self::wipe_set_has(&self.scene, &fade.out, name);
        if !incoming && !outgoing {
            return None;
        }
        let elapsed = fade.progress(self.now) * fade.dur_ms;
        let revealed = wipe::blind_strips(
            horizontal,
            span,
            appear_ms,
            reverse,
            elapsed,
            fade.dur_ms,
            screen_w as i32,
            screen_h as i32,
        );
        Some(if outgoing {
            wipe::blind_complement_strips(
                horizontal,
                span,
                &revealed,
                screen_w as i32,
                screen_h as i32,
            )
        } else {
            revealed
        })
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
        let extra: Vec<_> = self
            .scene
            .draw_order_with_back()
            .into_iter()
            .filter(|object| {
                !have.contains(&object.name)
                    && Self::wipe_set_has(&self.scene, &fade.inn, &object.name)
            })
            .cloned()
            .collect();
        ordered.splice(0..0, extra);
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
            // Desfase estable por nombre
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
