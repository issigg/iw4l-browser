use super::encode::{RecordCensus, RecordMesh};
use super::{ExactTessBind, PreparedExactDraw};
use crate::drawsurf::backend::{D3dScissorRect, SHADOWMAP_CLEAR_Z};
use crate::drawsurf::exact_pipeline::ExactPipelineRegistry;
use crate::drawsurf::shadowmap_sun_gpu::scissor_xywh;
use bevy::prelude::LinearRgba;
use bevy::render::render_phase::TrackedRenderPass;
use bevy::render::render_resource::{
    BindGroup, Buffer, CommandEncoder, IndexFormat, LoadOp, Operations, RenderPassColorAttachment,
    RenderPassDepthStencilAttachment, RenderPassDescriptor, StoreOp, TextureView,
};
use bevy::render::renderer::RenderDevice;
use render_frame::SunShadowViewport;
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
pub(super) struct ShadowGeometry<'a> {
    pub(super) world: RecordMesh<'a>,
    pub(super) smodel: RecordMesh<'a>,
    pub(super) cached: RecordMesh<'a>,
    pub(super) skinned: RecordMesh<'a>,
    pub(super) xmodel: RecordMesh<'a>,
    pub(super) smodel_index_epochs: &'a [Buffer],
    pub(super) xmodel_index_epochs: &'a [Buffer],
    pub(super) world_layer: Option<&'a Buffer>,
    pub(super) world_effect: Option<&'a Buffer>,
}

impl<'a> ShadowGeometry<'a> {
    fn mesh(&self, tess: ExactTessBind, epoch: u32) -> Option<RecordMesh<'a>> {
        let mut mesh = match tess {
            ExactTessBind::World => self.world,
            ExactTessBind::Smodel => self.smodel,
            ExactTessBind::SmodelCached => self.cached,
            ExactTessBind::SmodelSkinned => self.skinned,
            ExactTessBind::XModel => self.xmodel,
            _ => return None,
        };
        if tess == ExactTessBind::Smodel {
            mesh.index = self.smodel_index_epochs.get(epoch as usize);
        } else if tess == ExactTessBind::XModel {
            mesh.index = self.xmodel_index_epochs.get(epoch as usize);
        }
        Some(mesh)
    }
}

/// How group 1 (textures) is provided for a shadow pass.
#[derive(Clone, Copy)]
pub(super) enum ShadowTextureBinds<'a> {
    /// Native: one global bindless table bind group.
    Table(&'a BindGroup),
    /// Web: a bind group per draw built from the draw's slot words. `mask_sun` swaps the sun
    /// shadow map (the pass's own render target) for a placeholder.
    PerSlot {
        ports: &'a super::ExactColourPipeline,
        table: &'a super::texture_table::ExactTextureTable,
        mask_sun: bool,
    },
}

pub(super) struct ShadowPassInputs<'a> {
    pub(super) device: &'a RenderDevice,
    pub(super) registry: &'a ExactPipelineRegistry,
    pub(super) geometry: ShadowGeometry<'a>,
    pub(super) constants_bind: Option<&'a BindGroup>,
    pub(super) textures: ShadowTextureBinds<'a>,
    pub(super) color_view: &'a TextureView,
    pub(super) depth_view: &'a TextureView,
    pub(super) scissor: D3dScissorRect,
    pub(super) viewport: SunShadowViewport,
    pub(super) cleared: bool,
}

#[derive(Clone, Copy)]
enum ShadowRecordRefusal {
    PipelineNotReady,
    ConstantArenaMissing,
    UnsupportedTessKind,
    MissingTessBuffer,
}

impl ShadowRecordRefusal {
    const ALL: [Self; 4] = [
        Self::PipelineNotReady,
        Self::ConstantArenaMissing,
        Self::UnsupportedTessKind,
        Self::MissingTessBuffer,
    ];
    fn name(self) -> &'static str {
        match self {
            Self::PipelineNotReady => "PipelineNotReady",
            Self::ConstantArenaMissing => "ConstantArenaMissing",
            Self::UnsupportedTessKind => "UnsupportedTessKind",
            Self::MissingTessBuffer => "MissingTessBuffer",
        }
    }
}

#[derive(Default)]
pub(super) struct ShadowRecordResult {
    pub(super) indexed: u32,
    pub(super) bindings: RecordCensus,
    refusals: [u32; 4],
}

impl ShadowRecordResult {
    fn refuse(&mut self, cause: ShadowRecordRefusal) {
        let count = &mut self.refusals[cause as usize];
        *count = count.saturating_add(1);
    }

    pub(super) fn append_refusals(&self, total: &mut u32, rows: &mut BTreeMap<String, u32>) {
        for cause in ShadowRecordRefusal::ALL {
            let count = self.refusals[cause as usize];
            if count != 0 {
                *total = total.saturating_add(count);
                let row = rows.entry(cause.name().into()).or_insert(0);
                *row = row.saturating_add(count);
            }
        }
    }
}

pub(super) fn record_shadowmap_draws<'a>(
    encoder: &mut CommandEncoder,
    inputs: ShadowPassInputs<'_>,
    draws: impl IntoIterator<Item = &'a PreparedExactDraw>,
    label: &'static str,
) -> ShadowRecordResult {
    let ShadowPassInputs {
        device,
        registry,
        geometry,
        constants_bind,
        textures,
        color_view,
        depth_view,
        scissor,
        viewport,
        cleared,
    } = inputs;
    // Web path: the pass borrows its bind groups for its whole lifetime, so build them first.
    let draws: Vec<&'a PreparedExactDraw> = draws.into_iter().collect();
    let slot_groups: Vec<Option<BindGroup>> = match textures {
        ShadowTextureBinds::Table(_) => Vec::new(),
        ShadowTextureBinds::PerSlot {
            ports,
            table,
            mask_sun,
        } => {
            let mut groups = Vec::with_capacity(draws.len());
            let mut last: Option<(super::PortId, std::sync::Arc<[u32]>, Option<BindGroup>)> = None;
            let mut layout: Option<(
                super::PortId,
                bevy::render::render_resource::BindGroupLayout,
            )> = None;
            for draw in &draws {
                if let Some((port, slots, group)) = &last
                    && *port == draw.port
                    && (std::sync::Arc::ptr_eq(slots, &draw.texture_slots)
                        || **slots == *draw.texture_slots)
                {
                    groups.push(group.clone());
                    continue;
                }
                let group = ports.get(draw.port).and_then(|port| {
                    if layout.as_ref().map(|(id, _)| *id) != Some(draw.port) {
                        layout = Some((
                            draw.port,
                            registry.bind_group_layout(device, &port.textures_layout),
                        ));
                    }
                    table.slot_bind_group(
                        device,
                        &layout.as_ref().expect("set above").1,
                        &port.slot_regs,
                        &draw.texture_slots,
                        mask_sun,
                    )
                });
                last = Some((draw.port, draw.texture_slots.clone(), group.clone()));
                groups.push(group);
            }
            groups
        }
    };
    let mut result = ShadowRecordResult::default();
    let (sx, sy, sw, sh) = scissor_xywh(scissor);
    let vp = viewport;
    let attachments = [Some(RenderPassColorAttachment {
        view: color_view,
        resolve_target: None,
        ops: Operations {
            load: if cleared {
                LoadOp::Clear(LinearRgba::WHITE.into())
            } else {
                LoadOp::Load
            },
            store: StoreOp::Store,
        },
        depth_slice: None,
    })];
    let mut pass = TrackedRenderPass::new(
        device,
        encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some(label),
            color_attachments: &attachments,
            depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(Operations {
                    load: if cleared {
                        LoadOp::Clear(SHADOWMAP_CLEAR_Z)
                    } else {
                        LoadOp::Load
                    },
                    store: StoreOp::Store,
                }),
                stencil_ops: Some(Operations {
                    load: if cleared {
                        LoadOp::Clear(0)
                    } else {
                        LoadOp::Load
                    },
                    store: StoreOp::Store,
                }),
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        }),
    );
    if sw > 0 && sh > 0 {
        pass.set_scissor_rect(sx, sy, sw, sh);
    }
    let mut record_n = RecordCensus {
        state: u32::from(sw > 0 && sh > 0),
        ..RecordCensus::default()
    };
    let mut bound_pipeline = None;
    let mut bound_tess = None;
    let mut bound_epoch = None;
    let mut bound_depth = None;
    let mut constants_bound = false;
    let mut bound_slot_group: Option<bevy::render::render_resource::BindGroupId> = None;
    let mut indexed = 0u32;
    for (draw_index, draw) in draws.into_iter().enumerate() {
        let Some(gpu_pipeline) = registry.ready(draw.pipeline) else {
            result.refuse(ShadowRecordRefusal::PipelineNotReady);
            continue;
        };
        let (Some(constant_base), Some(constants_bind)) = (draw.constant_base, constants_bind)
        else {
            result.refuse(ShadowRecordRefusal::ConstantArenaMissing);
            continue;
        };
        let Some(mesh) = geometry.mesh(draw.tess, draw.ring_epoch) else {
            result.refuse(ShadowRecordRefusal::UnsupportedTessKind);
            continue;
        };
        let (vertex, index) = (mesh.vertex, mesh.index);
        let (Some(vertex), Some(index)) = (vertex, index) else {
            result.refuse(ShadowRecordRefusal::MissingTessBuffer);
            continue;
        };
        if bound_pipeline != Some(draw.pipeline)
            || bound_tess != Some(draw.tess)
            || bound_epoch != Some(draw.ring_epoch)
        {
            pass.set_render_pipeline(gpu_pipeline);
            pass.set_vertex_buffer(0, vertex.slice(..));
            if let Some(lighting) = mesh.lighting {
                pass.set_vertex_buffer(1, lighting.slice(..));
            }
            if draw.tess == ExactTessBind::World
                && let Some(effect) = geometry.world_effect
            {
                pass.set_vertex_buffer(2, effect.slice(..));
            }
            record_n.state = record_n.state.saturating_add(2);
            if draw.tess == ExactTessBind::World
                && let Some(layer) = geometry.world_layer
            {
                pass.set_vertex_buffer(1, layer.slice(..));
                record_n.state = record_n.state.saturating_add(1);
            }
            pass.set_index_buffer(index.slice(..), IndexFormat::Uint32);
            record_n.state = record_n.state.saturating_add(1);
            bound_pipeline = Some(draw.pipeline);
            bound_tess = Some(draw.tess);
            bound_epoch = Some(draw.ring_epoch);
        }

        if bound_depth != Some((draw.depth_min, draw.depth_max)) {
            pass.set_viewport(
                vp.x as f32,
                vp.y as f32,
                vp.width as f32,
                vp.height as f32,
                draw.depth_min,
                draw.depth_max,
            );
            bound_depth = Some((draw.depth_min, draw.depth_max));
            record_n.state = record_n.state.saturating_add(1);
        }
        if !constants_bound {
            pass.set_bind_group(0, constants_bind, &[]);
            constants_bound = true;
            record_n.group0 = record_n.group0.saturating_add(1);
            if let ShadowTextureBinds::Table(group) = textures {
                pass.set_bind_group(1, group, &[]);
                record_n.group1 = record_n.group1.saturating_add(1);
            }
        }
        if matches!(textures, ShadowTextureBinds::PerSlot { .. }) {
            let Some(group) = slot_groups.get(draw_index).and_then(Option::as_ref) else {
                result.refuse(ShadowRecordRefusal::PipelineNotReady);
                continue;
            };
            if bound_slot_group != Some(group.id()) {
                pass.set_bind_group(1, group, &[]);
                bound_slot_group = Some(group.id());
                record_n.group1 = record_n.group1.saturating_add(1);
            }
        }
        pass.draw_indexed(
            draw.start..draw.start.saturating_add(draw.count),
            0,
            constant_base..constant_base.saturating_add(1),
        );
        indexed = indexed.saturating_add(1);
    }
    drop(pass);
    result.indexed = indexed;
    result.bindings = record_n;
    result
}

pub(super) fn clear_empty_shadowmap_sun_partition_zero(
    encoder: &mut CommandEncoder,
    color_view: &bevy::render::render_resource::TextureView,
    depth_view: &bevy::render::render_resource::TextureView,
) {
    let attachments = [Some(RenderPassColorAttachment {
        view: color_view,
        resolve_target: None,
        ops: Operations {
            load: LoadOp::Clear(LinearRgba::WHITE.into()),
            store: StoreOp::Store,
        },
        depth_slice: None,
    })];
    let pass = encoder.begin_render_pass(&RenderPassDescriptor {
        label: Some("iw4_shadowmap_sun_partition_zero_empty"),
        color_attachments: &attachments,
        depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
            view: depth_view,
            depth_ops: Some(Operations {
                load: LoadOp::Clear(SHADOWMAP_CLEAR_Z),
                store: StoreOp::Store,
            }),
            stencil_ops: Some(Operations {
                load: LoadOp::Clear(0),
                store: StoreOp::Store,
            }),
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    drop(pass);
}
