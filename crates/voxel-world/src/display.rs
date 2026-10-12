//! VK_KHR_display presentation without a window server or a patched wgpu.
//!
//! Swapchain images are borrowed by wgpu through create_texture_from_hal. wgpu
//! owns the rendering and layout transitions; ash only acquires and presents.
//! Adapted from crates/chinese-chess/src/display.rs. One frame is in flight.
use anyhow::{Context, Result, bail, ensure};
use ash::{khr, vk};

pub fn create_instance() -> Result<wgpu::Instance> {
    let desc = wgpu_hal::InstanceDescriptor {
        name: "trimui-voxel-world",
        flags: wgpu::InstanceFlags::empty(),
        memory_budget_thresholds: Default::default(),
        backend_options: Default::default(),
        telemetry: None,
        display: None,
    };
    // SAFETY: The callback adds only an advertised extension. HAL owns the
    // resulting instance, then transfers that ownership to wgpu.
    let hal = unsafe {
        wgpu_hal::vulkan::Instance::init_with_callback(
            &desc,
            Some(Box::new(|args| {
                let Ok(supported) = args.entry.enumerate_instance_extension_properties(None) else {
                    return;
                };
                if supported
                    .iter()
                    .any(|ext| ext.extension_name_as_c_str().ok() == Some(khr::display::NAME))
                    && !args.extensions.contains(&khr::display::NAME)
                {
                    args.extensions.push(khr::display::NAME);
                }
            })),
        )
    }?;
    ensure!(
        hal.shared_instance()
            .extensions()
            .contains(&khr::display::NAME),
        "VK_KHR_display is unavailable"
    );
    Ok(unsafe { wgpu::Instance::from_hal::<wgpu_hal::api::Vulkan>(hal) })
}

pub struct DisplaySurface {
    loader: khr::surface::Instance,
    raw: vk::SurfaceKHR,
    pub extent: vk::Extent2D,
    _instance: wgpu::Instance,
}

impl DisplaySurface {
    pub fn new(instance: &wgpu::Instance, adapter: &wgpu::Adapter) -> Result<Self> {
        // SAFETY: Both HAL handles are borrowed from the same live wgpu instance.
        let hal_instance = unsafe { instance.as_hal::<wgpu_hal::api::Vulkan>() }
            .context("Not a Vulkan instance")?;
        let hal_adapter =
            unsafe { adapter.as_hal::<wgpu_hal::api::Vulkan>() }.context("Not a Vulkan adapter")?;
        let shared = hal_instance.shared_instance();
        let physical = hal_adapter.raw_physical_device();
        let display_api = khr::display::Instance::new(shared.entry(), shared.raw_instance());
        let displays = unsafe { display_api.get_physical_device_display_properties(physical) }?;
        let planes = unsafe { display_api.get_physical_device_display_plane_properties(physical) }?;
        for display in displays {
            let modes =
                unsafe { display_api.get_display_mode_properties(physical, display.display) }?;
            let Some(mode) = modes
                .iter()
                .find(|mode| mode.parameters.visible_region == display.physical_resolution)
                .or(modes.first())
            else {
                continue;
            };
            for (index, plane) in planes.iter().enumerate() {
                let supported = unsafe {
                    display_api.get_display_plane_supported_displays(physical, index as u32)
                }?;
                if !supported.contains(&display.display) {
                    continue;
                }
                ensure!(
                    display
                        .supported_transforms
                        .contains(vk::SurfaceTransformFlagsKHR::IDENTITY),
                    "Display does not support identity transform"
                );
                let caps = unsafe {
                    display_api.get_display_plane_capabilities(
                        physical,
                        mode.display_mode,
                        index as u32,
                    )
                }?;
                let alpha = [
                    vk::DisplayPlaneAlphaFlagsKHR::OPAQUE,
                    vk::DisplayPlaneAlphaFlagsKHR::GLOBAL,
                ]
                .into_iter()
                .find(|flag| caps.supported_alpha.contains(*flag))
                .context("No supported opaque/global plane alpha")?;
                let info = vk::DisplaySurfaceCreateInfoKHR::default()
                    .display_mode(mode.display_mode)
                    .plane_index(index as u32)
                    .plane_stack_index(plane.current_stack_index)
                    .transform(vk::SurfaceTransformFlagsKHR::IDENTITY)
                    .global_alpha(1.0)
                    .alpha_mode(alpha)
                    .image_extent(mode.parameters.visible_region);
                let raw = unsafe { display_api.create_display_plane_surface(&info, None) }
                    .context("Creating the native display surface")?;
                let extent = mode.parameters.visible_region;
                log::info!(
                    "Display: {}x{}, plane {index}; driver refresh_rate field: {}",
                    extent.width,
                    extent.height,
                    mode.parameters.refresh_rate
                );
                return Ok(Self {
                    loader: khr::surface::Instance::new(shared.entry(), shared.raw_instance()),
                    raw,
                    extent,
                    _instance: instance.clone(),
                });
            }
        }
        bail!("No compatible VK_KHR_display mode/plane found")
    }
}

impl Drop for DisplaySurface {
    fn drop(&mut self) {
        // No swapchain may outlive its surface; DirectDisplay owns this last.
        unsafe { self.loader.destroy_surface(self.raw, None) };
    }
}

pub struct Target {
    pub view: wgpu::TextureView,
    pub texture: wgpu::Texture,
}

pub struct DirectDisplay {
    pub targets: Vec<Target>,
    pub format: wgpu::TextureFormat,
    pub extent: vk::Extent2D,
    pub can_capture: bool,
    vk_device: ash::Device,
    loader: khr::swapchain::Device,
    swapchain: vk::SwapchainKHR,
    acquire_fence: vk::Fence,
    compat_wait: vk::Semaphore,
    queue: vk::Queue,
    _device: wgpu::Device,
    _surface: DisplaySurface,
}

impl DirectDisplay {
    pub fn new(surface: DisplaySurface, device: &wgpu::Device) -> Result<Self> {
        let hal =
            unsafe { device.as_hal::<wgpu_hal::api::Vulkan>() }.context("Not a Vulkan device")?;
        let physical = hal.raw_physical_device();
        ensure!(
            hal.enabled_device_extensions()
                .contains(&khr::swapchain::NAME),
            "VK_KHR_swapchain was not enabled"
        );
        ensure!(
            unsafe {
                surface.loader.get_physical_device_surface_support(
                    physical,
                    hal.queue_family_index(),
                    surface.raw,
                )
            }?,
            "wgpu graphics queue cannot present to this display"
        );
        let caps = unsafe {
            surface
                .loader
                .get_physical_device_surface_capabilities(physical, surface.raw)
        }?;
        let formats = unsafe {
            surface
                .loader
                .get_physical_device_surface_formats(physical, surface.raw)
        }?;
        let present_modes = unsafe {
            surface
                .loader
                .get_physical_device_surface_present_modes(physical, surface.raw)
        }?;
        log::info!(
            "Surface formats: {formats:?}; present modes: {present_modes:?}; usage: {:?}",
            caps.supported_usage_flags
        );
        let choice = formats
            .iter()
            .find(|f| f.format == vk::Format::B8G8R8A8_UNORM)
            .or_else(|| {
                formats
                    .iter()
                    .find(|f| f.format == vk::Format::R8G8B8A8_UNORM)
            })
            .context("Display has no supported 8-bit UNORM format")?;
        let format = if choice.format == vk::Format::B8G8R8A8_UNORM {
            wgpu::TextureFormat::Bgra8Unorm
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        };
        let extent = if caps.current_extent.width == u32::MAX {
            surface.extent
        } else {
            caps.current_extent
        };
        let image_count = (caps.min_image_count + 1).min(if caps.max_image_count == 0 {
            u32::MAX
        } else {
            caps.max_image_count
        });
        let composite = [
            vk::CompositeAlphaFlagsKHR::OPAQUE,
            vk::CompositeAlphaFlagsKHR::INHERIT,
            vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
            vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
        ]
        .into_iter()
        .find(|flag| caps.supported_composite_alpha.contains(*flag))
        .context("No supported composite alpha")?;
        let can_capture = caps
            .supported_usage_flags
            .contains(vk::ImageUsageFlags::TRANSFER_SRC);
        let vk_usage = vk::ImageUsageFlags::COLOR_ATTACHMENT
            | if can_capture {
                vk::ImageUsageFlags::TRANSFER_SRC
            } else {
                vk::ImageUsageFlags::empty()
            };
        ensure!(
            caps.supported_usage_flags.contains(vk_usage),
            "Display cannot be a render attachment"
        );
        let info = vk::SwapchainCreateInfoKHR::default()
            .surface(surface.raw)
            .min_image_count(image_count)
            .image_format(choice.format)
            .image_color_space(choice.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(vk_usage)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(caps.current_transform)
            .composite_alpha(composite)
            .present_mode(vk::PresentModeKHR::FIFO)
            .clipped(true);
        let vk_device = hal.raw_device().clone();
        let loader = khr::swapchain::Device::new(hal.shared_instance().raw_instance(), &vk_device);
        let swapchain = unsafe { loader.create_swapchain(&info, None) }
            .context("Creating the display swapchain")?;
        let mut result = Self {
            targets: Vec::new(),
            format,
            extent,
            can_capture,
            vk_device,
            loader,
            swapchain,
            acquire_fence: vk::Fence::null(),
            compat_wait: vk::Semaphore::null(),
            queue: hal.raw_queue(),
            _device: device.clone(),
            _surface: surface,
        };
        result.acquire_fence = unsafe {
            result
                .vk_device
                .create_fence(&vk::FenceCreateInfo::default(), None)
        }?;
        // The GE8300 driver dereferences pWaitSemaphoreValues even when every
        // wait is binary (a legal NULL in that case). An already-satisfied
        // timeline wait makes HAL populate values for the binary relay waits
        // as well, avoiding the driver's second-submission crash. No fork or
        // modification of the system Vulkan driver is required.
        let mut timeline = vk::SemaphoreTypeCreateInfo::default()
            .semaphore_type(vk::SemaphoreType::TIMELINE)
            .initial_value(0);
        let semaphore_info = vk::SemaphoreCreateInfo::default().push_next(&mut timeline);
        result.compat_wait = unsafe { result.vk_device.create_semaphore(&semaphore_info, None) }?;
        let images = unsafe { result.loader.get_swapchain_images(swapchain) }?;
        for image in images {
            let size = wgpu::Extent3d {
                width: extent.width,
                height: extent.height,
                depth_or_array_layers: 1,
            };
            let desc = wgpu::TextureDescriptor {
                label: Some("borrowed native swapchain image"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | if can_capture {
                        wgpu::TextureUsages::COPY_SRC
                    } else {
                        wgpu::TextureUsages::empty()
                    },
                view_formats: &[],
            };
            let hal_desc = wgpu_hal::TextureDescriptor {
                label: desc.label,
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: desc.dimension,
                format,
                usage: wgpu::TextureUses::COLOR_TARGET
                    | if can_capture {
                        wgpu::TextureUses::COPY_SRC
                    } else {
                        wgpu::TextureUses::empty()
                    },
                memory_flags: wgpu_hal::MemoryFlags::empty(),
                view_formats: Vec::new(),
            };
            // SAFETY: The image belongs to this exact Vulkan device and matches
            // the descriptors. External memory + a no-op callback prevent wgpu
            // from freeing swapchain-owned images. They start undefined. All
            // imported textures are released before destroying the swapchain.
            let texture = unsafe {
                let raw = hal.texture_from_raw(
                    image,
                    &hal_desc,
                    Some(Box::new(|| {})),
                    wgpu_hal::vulkan::TextureMemory::External,
                );
                device.create_texture_from_hal::<wgpu_hal::api::Vulkan>(
                    raw,
                    &desc,
                    wgpu::TextureUses::UNINITIALIZED,
                )
            };
            let view = texture.create_view(&Default::default());
            result.targets.push(Target { view, texture });
        }
        log::info!(
            "Swapchain ready: {} images, {format:?}, {}x{}",
            result.targets.len(),
            extent.width,
            extent.height
        );
        Ok(result)
    }

    pub fn acquire(&self) -> Result<usize> {
        // Host waits intentionally serialize this minimal demo. No Vulkan
        // semaphore is reused without waiting for its operation to complete.
        unsafe {
            self.vk_device.reset_fences(&[self.acquire_fence])?;
            let (index, _) = self.loader.acquire_next_image(
                self.swapchain,
                2_000_000_000,
                vk::Semaphore::null(),
                self.acquire_fence,
            )?;
            self.vk_device
                .wait_for_fences(&[self.acquire_fence], true, 2_000_000_000)?;
            Ok(index as usize)
        }
    }

    pub fn present(&self, index: usize) -> Result<()> {
        // Caller must finish the wgpu submission and transition to PRESENT.
        // This thread is the sole user of the shared graphics/present queue.
        let chains = [self.swapchain];
        let indices = [index as u32];
        let info = vk::PresentInfoKHR::default()
            .swapchains(&chains)
            .image_indices(&indices);
        unsafe {
            self.loader.queue_present(self.queue, &info)?;
            self.vk_device.queue_wait_idle(self.queue)?;
        }
        Ok(())
    }

    pub fn prepare_submit(&self, queue: &wgpu::Queue) -> Result<()> {
        let hal =
            unsafe { queue.as_hal::<wgpu_hal::api::Vulkan>() }.context("Not a Vulkan queue")?;
        hal.add_wait_semaphore(
            self.compat_wait,
            Some(0),
            vk::PipelineStageFlags::TOP_OF_PIPE,
        );
        Ok(())
    }
}

impl Drop for DirectDisplay {
    fn drop(&mut self) {
        // The swapchain owns VkImage allocation; destroy its borrowed views
        // before the swapchain, and keep the wgpu device and instance alive.
        unsafe {
            let _ = self.vk_device.device_wait_idle();
        }
        self.targets.clear();
        let _ = self._device.poll(wgpu::PollType::wait_indefinitely());
        unsafe {
            if self.compat_wait != vk::Semaphore::null() {
                self.vk_device.destroy_semaphore(self.compat_wait, None);
            }
            if self.acquire_fence != vk::Fence::null() {
                self.vk_device.destroy_fence(self.acquire_fence, None);
            }
            self.loader.destroy_swapchain(self.swapchain, None);
        }
    }
}
