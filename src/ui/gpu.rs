use eframe::egui_wgpu::{
    RenderState, SurfaceConfig, WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew,
};
use eframe::wgpu::{self, Backend, Backends, PresentMode};
use std::sync::Arc;

pub fn wgpu_options(backend: Option<&str>, present: Option<&str>) -> WgpuConfiguration {
    let mut setup = WgpuSetupCreateNew::without_display_handle();
    let egui_descriptor = setup.device_descriptor.clone();
    setup.device_descriptor = Arc::new(move |adapter| {
        let mut desc = egui_descriptor(adapter);
        desc.required_limits.max_texture_dimension_2d = adapter.limits().max_texture_dimension_2d;
        desc
    });
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

#[derive(Clone, Copy, Debug)]
pub struct MemoryBudget {
    pub budget: u64,
    pub used: u64,
}

#[derive(Clone)]
pub struct MemoryQuery {
    adapter: wgpu::Adapter,
    device: wgpu::Device,
}

impl MemoryQuery {
    pub fn new(render_state: &RenderState) -> Self {
        Self {
            adapter: render_state.adapter.clone(),
            device: render_state.device.clone(),
        }
    }

    pub fn budget(&self) -> Option<MemoryBudget> {
        match self.adapter.get_info().backend {
            #[cfg(target_vendor = "apple")]
            Backend::Metal => metal_budget(&self.device),
            #[cfg(windows)]
            Backend::Dx12 => dx12_budget(&self.adapter),
            #[cfg(any(windows, target_os = "linux"))]
            Backend::Vulkan => vulkan_budget(&self.device),
            _ => None,
        }
    }
}

#[cfg(target_vendor = "apple")]
fn metal_budget(device: &wgpu::Device) -> Option<MemoryBudget> {
    use objc2_metal::MTLDevice;
    // SAFETY: only reads properties of the device.
    let hal = unsafe { device.as_hal::<wgpu::hal::api::Metal>() }?;
    let raw = hal.raw_device();
    Some(MemoryBudget {
        budget: raw.recommendedMaxWorkingSetSize(),
        used: raw.currentAllocatedSize() as u64,
    })
}

#[cfg(windows)]
fn dx12_budget(adapter: &wgpu::Adapter) -> Option<MemoryBudget> {
    use windows::Win32::Graphics::Dxgi::DXGI_MEMORY_SEGMENT_GROUP_LOCAL;
    // SAFETY: only queries the adapter.
    let hal = unsafe { adapter.as_hal::<wgpu::hal::api::Dx12>() }?;
    // On integrated GPUs the local segment covers all memory the GPU may use.
    let info = (hal.raw_adapter())
        .query_video_memory_info(DXGI_MEMORY_SEGMENT_GROUP_LOCAL)
        .ok()?;
    Some(MemoryBudget {
        budget: info.Budget,
        used: info.CurrentUsage,
    })
}

#[cfg(any(windows, target_os = "linux"))]
fn vulkan_budget(device: &wgpu::Device) -> Option<MemoryBudget> {
    use ash::vk;
    // SAFETY: only queries the physical device.
    let hal = unsafe { device.as_hal::<wgpu::hal::api::Vulkan>() }?;
    let instance = hal.shared_instance();
    if !(hal.enabled_device_extensions()).contains(&ash::ext::memory_budget::NAME)
        || instance.instance_api_version() < vk::API_VERSION_1_1
    {
        return None;
    }
    let mut budget = vk::PhysicalDeviceMemoryBudgetPropertiesEXT::default();
    let mut props = vk::PhysicalDeviceMemoryProperties2::default().push_next(&mut budget);
    // SAFETY: Vulkan 1.1 and VK_EXT_memory_budget are available (checked above).
    unsafe {
        (instance.raw_instance())
            .get_physical_device_memory_properties2(hal.raw_physical_device(), &mut props);
    }
    let memory = props.memory_properties;
    // Textures go to the largest device-local heap.
    let (heap, _) = (memory.memory_heaps_as_slice().iter().enumerate())
        .filter(|(_, h)| h.flags.contains(vk::MemoryHeapFlags::DEVICE_LOCAL))
        .max_by_key(|(_, h)| h.size)?;
    Some(MemoryBudget {
        budget: budget.heap_budget[heap],
        used: budget.heap_usage[heap],
    })
}
