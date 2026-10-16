use eframe::egui_wgpu::{SurfaceConfig, WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew};
use eframe::wgpu::{self, Backend, Backends, PresentMode};
use std::sync::Arc;

pub fn wgpu_options(backend: Option<&str>, present: Option<&str>) -> WgpuConfiguration {
    let mut setup = WgpuSetupCreateNew::without_display_handle();
    let backends = match backend {
        Some("dx12") => Some(Backends::DX12),
        Some("vulkan") => Some(Backends::VULKAN),
        Some("gl") => Some(Backends::GL),
        Some("metal") => Some(Backends::METAL),
        _ => Backends::from_env(),
    };
    if let Some(b) = backends {
        setup.instance_descriptor.backends = b;
    } else if cfg!(windows) {
        setup.instance_descriptor.backends = Backends::VULKAN | Backends::DX12;
        setup.native_adapter_selector = Some(Arc::new(
            |adapters: &[wgpu::Adapter], surface: Option<&wgpu::Surface<'_>>| {
                adapters
                    .iter()
                    .filter(|a| surface.is_none_or(|s| a.is_surface_supported(s)))
                    .max_by_key(|a| {
                        let info = a.get_info();
                        (
                            info.backend == Backend::Vulkan,
                            info.device_type == wgpu::DeviceType::DiscreteGpu,
                        )
                    })
                    .cloned()
                    .ok_or_else(|| "no suitable GPU adapter found".to_string())
            },
        ));
    }
    let present_mode = match present {
        Some("mailbox") => PresentMode::Mailbox,
        Some("fifo" | "vsync") => PresentMode::AutoVsync,
        Some("immediate") => PresentMode::Immediate,
        Some("auto") => PresentMode::AutoNoVsync,
        _ if cfg!(windows) => PresentMode::AutoNoVsync,
        _ => PresentMode::AutoVsync,
    };
    WgpuConfiguration {
        surface: SurfaceConfig {
            present_mode,
            desired_maximum_frame_latency: Some(1),
        },
        wgpu_setup: WgpuSetup::CreateNew(setup),
        ..Default::default()
    }
}
