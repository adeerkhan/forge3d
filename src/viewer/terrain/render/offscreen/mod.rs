mod effects;
mod scene;
mod setup;

use super::*;

pub(super) struct SnapshotRenderState {
    pub(super) use_pbr: bool,
    pub(super) view_mat: glam::Mat4,
    pub(super) proj: glam::Mat4,
    pub(super) view_proj: glam::Mat4,
    pub(super) sun_dir: glam::Vec3,
    pub(super) eye: glam::Vec3,
    pub(super) render_origin_span: [f32; 4],
    pub(super) h_range: f32,
    pub(super) shader_z_scale: f32,
    pub(super) vo_view_proj: [[f32; 4]; 4],
    pub(super) vo_sun_dir: [f32; 3],
    pub(super) vo_lighting: [f32; 4],
}

impl ViewerTerrainScene {
    pub fn render_to_texture(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        target_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        selected_feature_id: u32,
        frame: crate::viewer::viewer_types::FrameCamera,
    ) -> Option<crate::core::resource_tracker::TrackedTexture> {
        self.snapshot_depth_texture = None;
        eprintln!("[DEBUG render_to_texture ENTRY] {}x{}", width, height);
        if self.terrain.is_none() {
            eprintln!("[DEBUG render_to_texture] No terrain, returning None");
            return None;
        }

        self.prepare_snapshot_resources(width, height);
        let clouds_enabled = self.pbr_config.clouds.enabled;
        let (color_tex, color_view) = match self.create_snapshot_color_target(
            "terrain_viewer.snapshot_color",
            target_format,
            width,
            height,
        ) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[terrain] failed to allocate snapshot color target: {e}");
                return None;
            }
        };
        // When clouds are enabled the terrain renders into a linear-HDR beauty
        // so the shared cloud pass can composite physically; the result is
        // tonemapped into `color_view` and then runs the normal postfx chain.
        let hdr = if clouds_enabled {
            match self.create_snapshot_hdr_target(width, height) {
                Ok(target) => Some(target),
                Err(e) => {
                    eprintln!("[terrain] failed to allocate snapshot HDR target: {e}");
                    None
                }
            }
        } else {
            None
        };
        let linear_hdr = hdr.is_some();
        let scene_view: &wgpu::TextureView = hdr
            .as_ref()
            .map(|(_, view)| view)
            .unwrap_or(&color_view);
        let (depth_tex, depth_view) = match self.create_snapshot_depth_target(width, height) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[terrain] failed to allocate snapshot depth target: {e}");
                return None;
            }
        };
        let state =
            self.build_snapshot_render_state(encoder, target_format, width, height, frame, linear_hdr);
        let has_vector_overlays = self.prepare_snapshot_overlays();

        self.render_snapshot_scene_pass(
            encoder,
            scene_view,
            &depth_view,
            selected_feature_id,
            &state,
            has_vector_overlays,
            linear_hdr,
        );
        self.render_snapshot_oit_pass(
            encoder,
            scene_view,
            &depth_view,
            width,
            height,
            selected_feature_id,
            &state,
            has_vector_overlays,
        );

        // Volumetric clouds: composite into the HDR beauty (linear), then the
        // viewer cloud renderer tonemaps it into `color_view`.
        if let Some((hdr_tex, hdr_view)) = hdr.as_ref() {
            let sun_intensity = self
                .terrain
                .as_ref()
                .map(|terrain| terrain.sun_intensity)
                .unwrap_or(1.0);
            let terrain_span = state.render_origin_span[2].abs();
            let elapsed = self.scatter_elapsed_time;
            self.render_viewer_clouds(
                encoder,
                hdr_tex,
                hdr_view,
                &depth_view,
                &color_view,
                width,
                height,
                target_format,
                state.view_proj,
                state.eye,
                state.sun_dir,
                [sun_intensity, sun_intensity, sun_intensity],
                terrain_span,
                elapsed,
            );
        }

        let output = self.apply_snapshot_effects(
            encoder,
            target_format,
            width,
            height,
            &depth_view,
            color_tex,
            color_view,
            &state,
        );
        self.snapshot_depth_texture = Some(depth_tex);
        Some(output)
    }
}
