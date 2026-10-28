use ash::{Entry, vk, vk::Handle};
use std::collections::HashMap;
use std::ffi::CStr;

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(feature = "nvidia-optical-flow")]
pub struct AdapterInfo {
    pub name: String,
    pub device_type: vk::PhysicalDeviceType,
    pub graphics_queue_family: u32,
    pub optical_flow_queue_family: Option<u32>,
    pub nvidia_optical_flow: bool,
}

#[cfg(feature = "nvidia-optical-flow")]
pub struct OpticalFlowDevice {
    _entry: Entry,
    instance: ash::Instance,
    device: ash::Device,
    pub optical_flow: ash::nv::optical_flow::Device,
    pub physical_device: vk::PhysicalDevice,
    pub queue: vk::Queue,
    pub queue_family_index: u32,
    pub name: String,
}

pub struct SdlVulkanContext {
    _entry: Entry,
    instance: ash::Instance,
    #[cfg(feature = "nvidia-optical-flow")]
    optical_flow_instance: ash::nv::optical_flow::Instance,
    surface_loader: ash::khr::surface::Instance,
    swapchain_loader: ash::khr::swapchain::Device,
    #[cfg(feature = "nvidia-optical-flow")]
    optical_flow_device: ash::nv::optical_flow::Device,
    device: ash::Device,
    pub surface: vk::SurfaceKHR,
    pub swapchain: vk::SwapchainKHR,
    pub swapchain_images: Vec<vk::Image>,
    pub swapchain_image_views: Vec<vk::ImageView>,
    pub framebuffers: Vec<vk::Framebuffer>,
    pub render_pass: vk::RenderPass,
    command_pool: vk::CommandPool,
    command_buffer: vk::CommandBuffer,
    image_acquired: vk::Semaphore,
    render_finished: Vec<vk::Semaphore>,
    frame_fence: vk::Fence,
    pub swapchain_format: vk::Format,
    pub swapchain_extent: vk::Extent2D,
    pub physical_device: vk::PhysicalDevice,
    pub graphics_queue: vk::Queue,
    #[cfg(feature = "nvidia-optical-flow")]
    pub optical_flow_queue: vk::Queue,
    pub graphics_queue_family: u32,
    #[cfg(feature = "nvidia-optical-flow")]
    pub optical_flow_queue_family: u32,
    pub name: String,
    native_swapchain_active: bool,
    #[cfg(feature = "nvidia-optical-flow")]
    optical_flow_session: Option<vk::OpticalFlowSessionNV>,
    #[cfg(feature = "nvidia-optical-flow")]
    optical_flow_session_info: Option<OpticalFlowSessionInfo>,
    #[cfg(feature = "nvidia-optical-flow")]
    optical_flow_images: Vec<VulkanImage>,
    #[cfg(feature = "nvidia-optical-flow")]
    optical_flow_staging_buffers: Vec<VulkanStagingBuffer>,
    #[cfg(feature = "nvidia-optical-flow")]
    optical_flow_probe_initialized: bool,
    scene_textures: HashMap<crate::renderer::TextureId, SceneTexture>,
    next_scene_texture_id: u64,
    scene_output: Option<crate::renderer::TextureId>,
    scene_fallback: Option<crate::renderer::TextureId>,
    scene_pipeline: Option<ScenePipeline>,
    scene_frame: Option<SceneFrameResources>,
    pending_texture_uploads: Vec<PendingTextureUpload>,
    retired_scene_textures: Vec<RetiredSceneTexture>,
    present_scale_filter: crate::scale_filter::ScaleFilter,
}

struct VulkanImage {
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
}

struct VulkanStagingBuffer {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    size: vk::DeviceSize,
}

struct SceneTexture {
    image: VulkanImage,
    width: u32,
    height: u32,
}

struct PendingTextureUpload {
    texture: crate::renderer::TextureId,
    staging: VulkanStagingBuffer,
    command_pool: vk::CommandPool,
    fence: vk::Fence,
}

struct RetiredSceneTexture {
    image: VulkanImage,
    fences: Vec<vk::Fence>,
    uploads: Vec<PendingTextureUpload>,
}

struct ScenePipeline {
    descriptor_set_layout: vk::DescriptorSetLayout,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    samplers: [vk::Sampler; 2],
}

struct SceneFrameResources {
    command_pool: vk::CommandPool,
    command_buffer: vk::CommandBuffer,
    descriptor_pool: vk::DescriptorPool,
    fence: vk::Fence,
    descriptor_capacity: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SceneDrawParameters {
    destination: [f32; 4],
    target_size: [f32; 2],
    dispatch_origin: [f32; 2],
    source_uv: [f32; 4],
    color: [f32; 4],
    flags: i32,
}

impl ScenePipeline {
    unsafe fn destroy(self, device: &ash::Device) {
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_descriptor_set_layout(self.descriptor_set_layout, None);
            for sampler in self.samplers {
                device.destroy_sampler(sampler, None);
            }
        }
    }
}

impl SceneFrameResources {
    fn new(device: &ash::Device, queue_family: u32) -> Result<Self, String> {
        let pool_info = vk::CommandPoolCreateInfo::default()
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
            .queue_family_index(queue_family);
        let command_pool = unsafe { device.create_command_pool(&pool_info, None) }
            .map_err(|error| format!("cannot create persistent scene command pool: {error:?}"))?;
        let command_buffer = match unsafe {
            device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(command_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )
        } {
            Ok(buffers) => buffers[0],
            Err(error) => {
                unsafe { device.destroy_command_pool(command_pool, None) };
                return Err(format!(
                    "cannot allocate persistent scene command buffer: {error:?}"
                ));
            }
        };
        let fence_info = vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED);
        let fence = match unsafe { device.create_fence(&fence_info, None) } {
            Ok(fence) => fence,
            Err(error) => {
                unsafe { device.destroy_command_pool(command_pool, None) };
                return Err(format!("cannot create persistent scene fence: {error:?}"));
            }
        };
        Ok(Self {
            command_pool,
            command_buffer,
            descriptor_pool: vk::DescriptorPool::null(),
            fence,
            descriptor_capacity: 0,
        })
    }

    fn ensure_descriptor_capacity(
        &mut self,
        device: &ash::Device,
        requested: u32,
    ) -> Result<(), String> {
        if self.descriptor_capacity >= requested {
            return Ok(());
        }
        let capacity = requested
            .checked_next_power_of_two()
            .ok_or_else(|| "scene descriptor capacity is too large".to_string())?;
        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                descriptor_count: capacity,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::STORAGE_IMAGE,
                descriptor_count: capacity,
            },
        ];
        let pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(capacity)
                    .pool_sizes(&pool_sizes),
                None,
            )
        }
        .map_err(|error| format!("cannot grow persistent scene descriptors: {error:?}"))?;
        unsafe {
            if self.descriptor_pool != vk::DescriptorPool::null() {
                device.destroy_descriptor_pool(self.descriptor_pool, None);
            }
        }
        self.descriptor_pool = pool;
        self.descriptor_capacity = capacity;
        Ok(())
    }

    unsafe fn destroy(self, device: &ash::Device) {
        unsafe {
            if self.descriptor_pool != vk::DescriptorPool::null() {
                device.destroy_descriptor_pool(self.descriptor_pool, None);
            }
            if self.fence != vk::Fence::null() {
                device.destroy_fence(self.fence, None);
            }
            if self.command_pool != vk::CommandPool::null() {
                device.destroy_command_pool(self.command_pool, None);
            }
        }
    }
}

struct SceneComputeResources<'a> {
    device: &'a ash::Device,
    descriptor_set_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    command_pool: vk::CommandPool,
    samplers: [vk::Sampler; 2],
}

fn create_scene_pipeline(device: &ash::Device) -> Result<ScenePipeline, String> {
    let mut resources = SceneComputeResources::new(device);
    let bindings = [
        vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE),
        vk::DescriptorSetLayoutBinding::default()
            .binding(1)
            .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE),
    ];
    resources.descriptor_set_layout = unsafe {
        device.create_descriptor_set_layout(
            &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
            None,
        )
    }
    .map_err(|error| format!("cannot create scene descriptor layout: {error:?}"))?;
    let sampler_info = |filter| {
        vk::SamplerCreateInfo::default()
            .mag_filter(filter)
            .min_filter(filter)
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .max_lod(0.0)
    };
    for (index, filter) in [vk::Filter::NEAREST, vk::Filter::LINEAR]
        .into_iter()
        .enumerate()
    {
        resources.samplers[index] =
            unsafe { device.create_sampler(&sampler_info(filter), None) }
                .map_err(|error| format!("cannot create scene sampler: {error:?}"))?;
    }
    let push_range = [vk::PushConstantRange::default()
        .stage_flags(vk::ShaderStageFlags::COMPUTE)
        .offset(0)
        .size(std::mem::size_of::<SceneDrawParameters>() as u32)];
    let layouts = [resources.descriptor_set_layout];
    resources.pipeline_layout = unsafe {
        device.create_pipeline_layout(
            &vk::PipelineLayoutCreateInfo::default()
                .set_layouts(&layouts)
                .push_constant_ranges(&push_range),
            None,
        )
    }
    .map_err(|error| format!("cannot create scene pipeline layout: {error:?}"))?;
    let shader_bytes = include_bytes!("../shaders/scene.comp.spv");
    let shader_words = shader_bytes
        .chunks_exact(4)
        .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        .collect::<Vec<_>>();
    let shader = unsafe {
        device.create_shader_module(
            &vk::ShaderModuleCreateInfo::default().code(&shader_words),
            None,
        )
    }
    .map_err(|error| format!("cannot load scene compute shader: {error:?}"))?;
    let entry = c"main";
    let stages = [vk::PipelineShaderStageCreateInfo::default()
        .stage(vk::ShaderStageFlags::COMPUTE)
        .module(shader)
        .name(entry)];
    let pipeline_info = [vk::ComputePipelineCreateInfo::default()
        .stage(stages[0])
        .layout(resources.pipeline_layout)];
    let pipeline_result =
        unsafe { device.create_compute_pipelines(vk::PipelineCache::null(), &pipeline_info, None) };
    unsafe { device.destroy_shader_module(shader, None) };
    resources.pipeline = pipeline_result
        .map_err(|(_, error)| format!("cannot create scene compute pipeline: {error:?}"))?[0];
    let result = ScenePipeline {
        descriptor_set_layout: resources.descriptor_set_layout,
        pipeline_layout: resources.pipeline_layout,
        pipeline: resources.pipeline,
        samplers: resources.samplers,
    };
    resources.descriptor_set_layout = vk::DescriptorSetLayout::null();
    resources.pipeline_layout = vk::PipelineLayout::null();
    resources.pipeline = vk::Pipeline::null();
    resources.samplers = [vk::Sampler::null(); 2];
    Ok(result)
}

impl<'a> SceneComputeResources<'a> {
    fn new(device: &'a ash::Device) -> Self {
        Self {
            device,
            descriptor_set_layout: vk::DescriptorSetLayout::null(),
            descriptor_pool: vk::DescriptorPool::null(),
            pipeline_layout: vk::PipelineLayout::null(),
            pipeline: vk::Pipeline::null(),
            command_pool: vk::CommandPool::null(),
            samplers: [vk::Sampler::null(); 2],
        }
    }
}

impl Drop for SceneComputeResources<'_> {
    fn drop(&mut self) {
        unsafe {
            if self.command_pool != vk::CommandPool::null() {
                self.device.destroy_command_pool(self.command_pool, None);
            }
            if self.pipeline != vk::Pipeline::null() {
                self.device.destroy_pipeline(self.pipeline, None);
            }
            if self.pipeline_layout != vk::PipelineLayout::null() {
                self.device
                    .destroy_pipeline_layout(self.pipeline_layout, None);
            }
            if self.descriptor_pool != vk::DescriptorPool::null() {
                self.device
                    .destroy_descriptor_pool(self.descriptor_pool, None);
            }
            if self.descriptor_set_layout != vk::DescriptorSetLayout::null() {
                self.device
                    .destroy_descriptor_set_layout(self.descriptor_set_layout, None);
            }
            for sampler in self.samplers {
                if sampler != vk::Sampler::null() {
                    self.device.destroy_sampler(sampler, None);
                }
            }
        }
    }
}

#[cfg(feature = "nvidia-optical-flow")]
struct InterpolationProbeResources<'a> {
    device: &'a ash::Device,
    descriptor_set_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    sampler: vk::Sampler,
    command_pool: vk::CommandPool,
    fence: vk::Fence,
}

#[cfg(feature = "nvidia-optical-flow")]
impl<'a> InterpolationProbeResources<'a> {
    fn new(device: &'a ash::Device) -> Self {
        Self {
            device,
            descriptor_set_layout: vk::DescriptorSetLayout::null(),
            descriptor_pool: vk::DescriptorPool::null(),
            pipeline_layout: vk::PipelineLayout::null(),
            pipeline: vk::Pipeline::null(),
            sampler: vk::Sampler::null(),
            command_pool: vk::CommandPool::null(),
            fence: vk::Fence::null(),
        }
    }
}

#[cfg(feature = "nvidia-optical-flow")]
impl Drop for InterpolationProbeResources<'_> {
    fn drop(&mut self) {
        if self.fence != vk::Fence::null()
            && let Err(error) = unsafe { self.device.device_wait_idle() }
        {
            log::warn!("cannot wait for interpolation probe resources: {error:?}");
        }
        unsafe {
            if self.fence != vk::Fence::null() {
                self.device.destroy_fence(self.fence, None);
            }
            if self.command_pool != vk::CommandPool::null() {
                self.device.destroy_command_pool(self.command_pool, None);
            }
            if self.pipeline != vk::Pipeline::null() {
                self.device.destroy_pipeline(self.pipeline, None);
            }
            if self.pipeline_layout != vk::PipelineLayout::null() {
                self.device
                    .destroy_pipeline_layout(self.pipeline_layout, None);
            }
            if self.descriptor_pool != vk::DescriptorPool::null() {
                self.device
                    .destroy_descriptor_pool(self.descriptor_pool, None);
            }
            if self.descriptor_set_layout != vk::DescriptorSetLayout::null() {
                self.device
                    .destroy_descriptor_set_layout(self.descriptor_set_layout, None);
            }
            if self.sampler != vk::Sampler::null() {
                self.device.destroy_sampler(self.sampler, None);
            }
        }
    }
}

fn create_vulkan_image(
    instance: &ash::Instance,
    device: &ash::Device,
    physical_device: vk::PhysicalDevice,
    width: u32,
    height: u32,
    format: vk::Format,
    image_usage: vk::ImageUsageFlags,
    queue_families: &[u32],
) -> Result<VulkanImage, String> {
    let sharing_mode = if queue_families.len() > 1 {
        vk::SharingMode::CONCURRENT
    } else {
        vk::SharingMode::EXCLUSIVE
    };
    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(format)
        .extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(image_usage)
        .sharing_mode(sharing_mode)
        .queue_family_indices(if sharing_mode == vk::SharingMode::CONCURRENT {
            queue_families
        } else {
            &[]
        })
        .initial_layout(vk::ImageLayout::UNDEFINED);
    allocate_vulkan_image(instance, device, physical_device, image_info, format)
}

#[cfg(feature = "nvidia-optical-flow")]
#[cfg(feature = "nvidia-optical-flow")]
fn create_optical_flow_image(
    instance: &ash::Instance,
    device: &ash::Device,
    physical_device: vk::PhysicalDevice,
    width: u32,
    height: u32,
    format: vk::Format,
    optical_flow_usage: vk::OpticalFlowUsageFlagsNV,
    image_usage: vk::ImageUsageFlags,
    queue_families: &[u32],
) -> Result<VulkanImage, String> {
    let sharing_mode = if queue_families.len() > 1 {
        vk::SharingMode::CONCURRENT
    } else {
        vk::SharingMode::EXCLUSIVE
    };
    let mut image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(format)
        .extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(image_usage)
        .sharing_mode(sharing_mode)
        .queue_family_indices(if sharing_mode == vk::SharingMode::CONCURRENT {
            queue_families
        } else {
            &[]
        })
        .initial_layout(vk::ImageLayout::UNDEFINED);
    let mut flow_info = vk::OpticalFlowImageFormatInfoNV::default().usage(optical_flow_usage);
    image_info = image_info.push_next(&mut flow_info);
    allocate_vulkan_image(instance, device, physical_device, image_info, format)
}

fn allocate_vulkan_image(
    instance: &ash::Instance,
    device: &ash::Device,
    physical_device: vk::PhysicalDevice,
    image_info: vk::ImageCreateInfo<'_>,
    format: vk::Format,
) -> Result<VulkanImage, String> {
    let image = unsafe { device.create_image(&image_info, None) }
        .map_err(|error| format!("cannot create Vulkan image: {error:?}"))?;
    let requirements = unsafe { device.get_image_memory_requirements(image) };
    let memory_properties =
        unsafe { instance.get_physical_device_memory_properties(physical_device) };
    let memory_type_index = (0..memory_properties.memory_type_count)
        .filter(|index| requirements.memory_type_bits & (1 << index) != 0)
        .find(|index| {
            memory_properties.memory_types[*index as usize]
                .property_flags
                .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
        })
        .or_else(|| {
            (0..memory_properties.memory_type_count)
                .find(|index| requirements.memory_type_bits & (1 << index) != 0)
        });
    let Some(memory_type_index) = memory_type_index else {
        unsafe { device.destroy_image(image, None) };
        return Err("no compatible Vulkan memory type for image".into());
    };
    let allocation_info = vk::MemoryAllocateInfo::default()
        .allocation_size(requirements.size)
        .memory_type_index(memory_type_index);
    let memory = match unsafe { device.allocate_memory(&allocation_info, None) } {
        Ok(memory) => memory,
        Err(error) => {
            unsafe { device.destroy_image(image, None) };
            return Err(format!("cannot allocate Vulkan image memory: {error:?}"));
        }
    };
    if let Err(error) = unsafe { device.bind_image_memory(image, memory, 0) } {
        unsafe {
            device.free_memory(memory, None);
            device.destroy_image(image, None);
        }
        return Err(format!("cannot bind Vulkan image memory: {error:?}"));
    }
    let view_info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(format)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        });
    let view = match unsafe { device.create_image_view(&view_info, None) } {
        Ok(view) => view,
        Err(error) => {
            unsafe {
                device.free_memory(memory, None);
                device.destroy_image(image, None);
            }
            return Err(format!("cannot create Vulkan image view: {error:?}"));
        }
    };
    Ok(VulkanImage {
        image,
        memory,
        view,
    })
}

unsafe fn destroy_vulkan_image(device: &ash::Device, image: VulkanImage) {
    unsafe {
        device.destroy_image_view(image.view, None);
        device.destroy_image(image.image, None);
        device.free_memory(image.memory, None);
    }
}

fn create_staging_buffer(
    instance: &ash::Instance,
    device: &ash::Device,
    physical_device: vk::PhysicalDevice,
    size: vk::DeviceSize,
    queue_families: &[u32],
) -> Result<VulkanStagingBuffer, String> {
    let sharing_mode = if queue_families.len() > 1 {
        vk::SharingMode::CONCURRENT
    } else {
        vk::SharingMode::EXCLUSIVE
    };
    let buffer_info = vk::BufferCreateInfo::default()
        .size(size)
        .usage(
            vk::BufferUsageFlags::TRANSFER_SRC
                | vk::BufferUsageFlags::TRANSFER_DST
                | vk::BufferUsageFlags::STORAGE_BUFFER,
        )
        .sharing_mode(sharing_mode)
        .queue_family_indices(if sharing_mode == vk::SharingMode::CONCURRENT {
            queue_families
        } else {
            &[]
        });
    let buffer = unsafe { device.create_buffer(&buffer_info, None) }
        .map_err(|error| format!("cannot create Vulkan staging buffer: {error:?}"))?;
    let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
    let memory_properties =
        unsafe { instance.get_physical_device_memory_properties(physical_device) };
    let memory_type_index = (0..memory_properties.memory_type_count).find(|index| {
        requirements.memory_type_bits & (1 << index) != 0
            && memory_properties.memory_types[*index as usize]
                .property_flags
                .contains(
                    vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
                )
    });
    let Some(memory_type_index) = memory_type_index else {
        unsafe { device.destroy_buffer(buffer, None) };
        return Err("no host-coherent Vulkan staging memory type".into());
    };
    let allocation_info = vk::MemoryAllocateInfo::default()
        .allocation_size(requirements.size)
        .memory_type_index(memory_type_index);
    let memory = match unsafe { device.allocate_memory(&allocation_info, None) } {
        Ok(memory) => memory,
        Err(error) => {
            unsafe { device.destroy_buffer(buffer, None) };
            return Err(format!("cannot allocate Vulkan staging memory: {error:?}"));
        }
    };
    if let Err(error) = unsafe { device.bind_buffer_memory(buffer, memory, 0) } {
        unsafe {
            device.free_memory(memory, None);
            device.destroy_buffer(buffer, None);
        }
        return Err(format!("cannot bind Vulkan staging memory: {error:?}"));
    }
    Ok(VulkanStagingBuffer {
        buffer,
        memory,
        size,
    })
}

fn write_staging_buffer(
    device: &ash::Device,
    buffer: &VulkanStagingBuffer,
    bytes: &[u8],
) -> Result<(), String> {
    if bytes.len() as u64 > buffer.size {
        return Err("Vulkan staging data exceeds its buffer".into());
    }
    let mapped =
        unsafe { device.map_memory(buffer.memory, 0, buffer.size, vk::MemoryMapFlags::empty()) }
            .map_err(|error| format!("cannot map Vulkan staging memory: {error:?}"))?;
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), mapped.cast::<u8>(), bytes.len());
        device.unmap_memory(buffer.memory);
    }
    Ok(())
}

#[cfg(any(test, feature = "nvidia-optical-flow"))]
fn read_staging_buffer(
    device: &ash::Device,
    buffer: &VulkanStagingBuffer,
) -> Result<Vec<u8>, String> {
    let mapped =
        unsafe { device.map_memory(buffer.memory, 0, buffer.size, vk::MemoryMapFlags::empty()) }
            .map_err(|error| format!("cannot map optical-flow output memory: {error:?}"))?;
    let mut bytes = vec![0; buffer.size as usize];
    unsafe {
        std::ptr::copy_nonoverlapping(mapped.cast::<u8>(), bytes.as_mut_ptr(), bytes.len());
        device.unmap_memory(buffer.memory);
    }
    Ok(bytes)
}

#[cfg(any(test, feature = "nvidia-optical-flow"))]
#[allow(dead_code)]
fn submit_and_wait(
    device: &ash::Device,
    queue: vk::Queue,
    command_buffer: vk::CommandBuffer,
    operation: &str,
) -> Result<(), String> {
    let fence = unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) }
        .map_err(|error| format!("cannot create {operation} fence: {error:?}"))?;
    let command_buffers = [command_buffer];
    let submit_result = unsafe {
        device.queue_submit(
            queue,
            &[vk::SubmitInfo::default().command_buffers(&command_buffers)],
            fence,
        )
    };
    if let Err(error) = submit_result {
        unsafe { device.destroy_fence(fence, None) };
        return Err(format!("cannot submit {operation}: {error:?}"));
    }
    let wait_result = unsafe { device.wait_for_fences(&[fence], true, u64::MAX) };
    unsafe { device.destroy_fence(fence, None) };
    wait_result.map_err(|error| format!("cannot wait for {operation}: {error:?}"))
}

fn enqueue_texture_upload(
    device: &ash::Device,
    queue: vk::Queue,
    queue_family: u32,
    image: vk::Image,
    width: u32,
    height: u32,
    staging: &VulkanStagingBuffer,
    old_layout: vk::ImageLayout,
) -> Result<(vk::CommandPool, vk::Fence), String> {
    let pool_info = vk::CommandPoolCreateInfo::default()
        .flags(vk::CommandPoolCreateFlags::TRANSIENT)
        .queue_family_index(queue_family);
    let command_pool = unsafe { device.create_command_pool(&pool_info, None) }
        .map_err(|error| format!("cannot create scene texture upload pool: {error:?}"))?;
    let result = (|| {
        let command_buffer = unsafe {
            device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(command_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )
        }
        .map_err(|error| {
            format!("cannot allocate scene texture upload command buffer: {error:?}")
        })?[0];
        unsafe {
            device
                .begin_command_buffer(command_buffer, &vk::CommandBufferBeginInfo::default())
                .map_err(|error| format!("cannot begin scene texture upload: {error:?}"))?;
            let range = vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            };
            let to_copy = [vk::ImageMemoryBarrier2::default()
                .src_stage_mask(if old_layout == vk::ImageLayout::UNDEFINED {
                    vk::PipelineStageFlags2::NONE
                } else {
                    vk::PipelineStageFlags2::COMPUTE_SHADER
                        | vk::PipelineStageFlags2::FRAGMENT_SHADER
                })
                .src_access_mask(if old_layout == vk::ImageLayout::UNDEFINED {
                    vk::AccessFlags2::empty()
                } else {
                    vk::AccessFlags2::SHADER_SAMPLED_READ
                })
                .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .old_layout(old_layout)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .subresource_range(range)];
            device.cmd_pipeline_barrier2(
                command_buffer,
                &vk::DependencyInfo::default().image_memory_barriers(&to_copy),
            );
            let region = [vk::BufferImageCopy::default()
                .image_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .image_extent(vk::Extent3D {
                    width,
                    height,
                    depth: 1,
                })];
            device.cmd_copy_buffer_to_image(
                command_buffer,
                staging.buffer,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &region,
            );
            let to_sample = [vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .dst_stage_mask(
                    vk::PipelineStageFlags2::FRAGMENT_SHADER
                        | vk::PipelineStageFlags2::COMPUTE_SHADER,
                )
                .dst_access_mask(
                    vk::AccessFlags2::SHADER_SAMPLED_READ | vk::AccessFlags2::SHADER_STORAGE_READ,
                )
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .subresource_range(range)];
            device.cmd_pipeline_barrier2(
                command_buffer,
                &vk::DependencyInfo::default().image_memory_barriers(&to_sample),
            );
            device
                .end_command_buffer(command_buffer)
                .map_err(|error| format!("cannot finish scene texture upload: {error:?}"))?;
        }
        let fence = unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) }
            .map_err(|error| format!("cannot create scene texture upload fence: {error:?}"))?;
        let command_buffers = [command_buffer];
        if let Err(error) = unsafe {
            device.queue_submit(
                queue,
                &[vk::SubmitInfo::default().command_buffers(&command_buffers)],
                fence,
            )
        } {
            unsafe { device.destroy_fence(fence, None) };
            return Err(format!("cannot submit scene texture upload: {error:?}"));
        }
        Ok(fence)
    })();
    match result {
        Ok(fence) => Ok((command_pool, fence)),
        Err(error) => {
            unsafe { device.destroy_command_pool(command_pool, None) };
            Err(error)
        }
    }
}

#[cfg(feature = "nvidia-optical-flow")]
fn make_translation_probe_frame(width: u32, height: u32, x_offset: u32) -> Vec<u8> {
    let mut pixels = vec![0; (width * height * 4) as usize];
    let sprite_origin = (width - 48) / 2 + x_offset;
    let sprite_top = (height - 48) / 2;
    for y in 0..height {
        for x in 0..width {
            let background_hash = (x / 8).wrapping_mul(0x9E37_79B9).rotate_left(7)
                ^ (y / 8).wrapping_mul(0x85EB_CA6B);
            let mut luminance =
                if (x % 53 < 2) || (y % 47 < 2) || background_hash & 0x8000_0000 != 0 {
                    32
                } else {
                    8
                };
            if (sprite_origin..sprite_origin + 48).contains(&x)
                && (sprite_top..sprite_top + 48).contains(&y)
            {
                let local_x = x - sprite_origin;
                let local_y = y - sprite_top;
                let sprite_hash = (local_x / 2).wrapping_mul(0x9E37_79B9).rotate_left(7)
                    ^ (local_y / 2).wrapping_mul(0x85EB_CA6B);
                let outline = local_x < 2 || local_x >= 46 || local_y < 2 || local_y >= 46;
                let anime_hatch = (local_x + local_y * 3) % 17 < 2;
                luminance = if outline || anime_hatch || sprite_hash & 0x8000_0000 != 0 {
                    240
                } else {
                    72
                };
            }
            let offset = ((y * width + x) * 4) as usize;
            pixels[offset..offset + 4].copy_from_slice(&[luminance, luminance, luminance, 255]);
        }
    }
    pixels
}

#[cfg(feature = "nvidia-optical-flow")]
fn mean_probe_flow(vectors: &[u8], width: u32, height: u32) -> Result<(f32, f32), String> {
    let mut flow_x = 0.0_f32;
    let mut flow_y = 0.0_f32;
    let mut sample_count = 0;
    let center_x = width / 2;
    let center_y = height / 2;
    for row in center_y.saturating_sub(6)..(center_y + 6).min(height) {
        for column in center_x.saturating_sub(6)..(center_x + 6).min(width) {
            let offset = ((row * width + column) * 4) as usize;
            let bytes = vectors
                .get(offset..offset + 4)
                .ok_or("optical-flow output is smaller than its expected grid")?;
            let x = i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / 32.0;
            let y = i16::from_le_bytes([bytes[2], bytes[3]]) as f32 / 32.0;
            flow_x += x;
            flow_y += y;
            sample_count += 1;
        }
    }
    if sample_count == 0 {
        return Err("optical-flow probe produced no interior vector samples".into());
    }
    Ok((flow_x / sample_count as f32, flow_y / sample_count as f32))
}

#[cfg(feature = "nvidia-optical-flow")]
fn optical_flow_input_pixels(
    pixels: &[u8],
    format: vk::Format,
) -> Result<std::borrow::Cow<'_, [u8]>, String> {
    match format {
        vk::Format::R8G8B8A8_UNORM => Ok(std::borrow::Cow::Borrowed(pixels)),
        vk::Format::B8G8R8A8_UNORM => {
            if pixels.len() % 4 != 0 {
                return Err("Optical Flow input must contain complete RGBA pixels".into());
            }
            let mut bgra = pixels.to_vec();
            for pixel in bgra.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            Ok(std::borrow::Cow::Owned(bgra))
        }
        _ => Err(format!(
            "unsupported Optical Flow input format {format:?}; expected linear RGBA8 or BGRA8"
        )),
    }
}

#[cfg(feature = "nvidia-optical-flow")]
fn interpolate_frames_on_gpu(
    device: &ash::Device,
    queue: vk::Queue,
    queue_family: u32,
    dimensions: vk::Extent2D,
    input_images: [vk::Image; 3],
    input_views: [vk::ImageView; 2],
    flow_vectors: &VulkanStagingBuffer,
    output_image: vk::Image,
    output_view: vk::ImageView,
    readback_buffer: &VulkanStagingBuffer,
    flow_complete: vk::Semaphore,
    output_initialized: bool,
) -> Result<Vec<u8>, String> {
    let mut resources = InterpolationProbeResources::new(device);
    let bindings = [
        vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE),
        vk::DescriptorSetLayoutBinding::default()
            .binding(1)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE),
        vk::DescriptorSetLayoutBinding::default()
            .binding(2)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE),
        vk::DescriptorSetLayoutBinding::default()
            .binding(3)
            .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE),
    ];
    let descriptor_layout_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
    resources.descriptor_set_layout =
        unsafe { device.create_descriptor_set_layout(&descriptor_layout_info, None) }
            .map_err(|error| format!("cannot create interpolation descriptor layout: {error:?}"))?;
    let sampler_info = vk::SamplerCreateInfo::default()
        .mag_filter(vk::Filter::LINEAR)
        .min_filter(vk::Filter::LINEAR)
        .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
        .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
        .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
        .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
        .max_lod(0.0);
    resources.sampler = unsafe { device.create_sampler(&sampler_info, None) }
        .map_err(|error| format!("cannot create interpolation sampler: {error:?}"))?;
    let pool_sizes = [
        vk::DescriptorPoolSize {
            ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
            descriptor_count: 2,
        },
        vk::DescriptorPoolSize {
            ty: vk::DescriptorType::STORAGE_BUFFER,
            descriptor_count: 1,
        },
        vk::DescriptorPoolSize {
            ty: vk::DescriptorType::STORAGE_IMAGE,
            descriptor_count: 1,
        },
    ];
    let pool_info = vk::DescriptorPoolCreateInfo::default()
        .max_sets(1)
        .pool_sizes(&pool_sizes);
    resources.descriptor_pool = unsafe { device.create_descriptor_pool(&pool_info, None) }
        .map_err(|error| format!("cannot create interpolation descriptor pool: {error:?}"))?;
    let set_layouts = [resources.descriptor_set_layout];
    let set_info = vk::DescriptorSetAllocateInfo::default()
        .descriptor_pool(resources.descriptor_pool)
        .set_layouts(&set_layouts);
    let descriptor_set = unsafe { device.allocate_descriptor_sets(&set_info) }
        .map_err(|error| format!("cannot allocate interpolation descriptors: {error:?}"))?[0];
    let sampled_infos = input_views.map(|view| {
        vk::DescriptorImageInfo::default()
            .sampler(resources.sampler)
            .image_view(view)
            .image_layout(vk::ImageLayout::GENERAL)
    });
    let flow_buffer_info = [vk::DescriptorBufferInfo::default()
        .buffer(flow_vectors.buffer)
        .offset(0)
        .range(flow_vectors.size)];
    let output_image_info = [vk::DescriptorImageInfo::default()
        .image_view(output_view)
        .image_layout(vk::ImageLayout::GENERAL)];
    let write_infos = [
        vk::WriteDescriptorSet::default()
            .dst_set(descriptor_set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(&sampled_infos[0..1]),
        vk::WriteDescriptorSet::default()
            .dst_set(descriptor_set)
            .dst_binding(1)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(&sampled_infos[1..2]),
        vk::WriteDescriptorSet::default()
            .dst_set(descriptor_set)
            .dst_binding(2)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(&flow_buffer_info),
        vk::WriteDescriptorSet::default()
            .dst_set(descriptor_set)
            .dst_binding(3)
            .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
            .image_info(&output_image_info),
    ];
    unsafe { device.update_descriptor_sets(&write_infos, &[]) };
    let pipeline_layout_info = vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts);
    resources.pipeline_layout =
        unsafe { device.create_pipeline_layout(&pipeline_layout_info, None) }
            .map_err(|error| format!("cannot create interpolation pipeline layout: {error:?}"))?;

    let shader_bytes = include_bytes!("../shaders/interpolate.comp.spv");
    if shader_bytes.len() % 4 != 0 {
        return Err("interpolation SPIR-V bytecode has an invalid length".into());
    }
    let shader_words = shader_bytes
        .chunks_exact(4)
        .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        .collect::<Vec<_>>();
    let shader_info = vk::ShaderModuleCreateInfo::default().code(&shader_words);
    let shader_module = unsafe { device.create_shader_module(&shader_info, None) }
        .map_err(|error| format!("cannot load interpolation compute shader: {error:?}"))?;
    let entry_point = c"main";
    let stage = vk::PipelineShaderStageCreateInfo::default()
        .stage(vk::ShaderStageFlags::COMPUTE)
        .module(shader_module)
        .name(entry_point);
    let pipeline_info = [vk::ComputePipelineCreateInfo::default()
        .stage(stage)
        .layout(resources.pipeline_layout)];
    let pipeline_result =
        unsafe { device.create_compute_pipelines(vk::PipelineCache::null(), &pipeline_info, None) };
    unsafe { device.destroy_shader_module(shader_module, None) };
    resources.pipeline = match pipeline_result {
        Ok(pipelines) => pipelines[0],
        Err((_, error)) => {
            return Err(format!(
                "cannot create interpolation compute pipeline: {error:?}"
            ));
        }
    };

    let command_pool_info = vk::CommandPoolCreateInfo::default()
        .flags(vk::CommandPoolCreateFlags::TRANSIENT)
        .queue_family_index(queue_family);
    resources.command_pool = unsafe { device.create_command_pool(&command_pool_info, None) }
        .map_err(|error| format!("cannot create interpolation command pool: {error:?}"))?;
    let command_buffer_info = vk::CommandBufferAllocateInfo::default()
        .command_pool(resources.command_pool)
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(1);
    let command_buffer = unsafe { device.allocate_command_buffers(&command_buffer_info) }
        .map_err(|error| format!("cannot allocate interpolation command buffer: {error:?}"))?[0];
    unsafe { device.begin_command_buffer(command_buffer, &vk::CommandBufferBeginInfo::default()) }
        .map_err(|error| format!("cannot begin interpolation command buffer: {error:?}"))?;

    let image_range = vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        base_mip_level: 0,
        level_count: 1,
        base_array_layer: 0,
        layer_count: 1,
    };
    let input_barriers = input_images
        .iter()
        .take(2)
        .map(|image| {
            vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::NONE)
                .src_access_mask(vk::AccessFlags2::empty())
                .dst_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
                .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
                .old_layout(vk::ImageLayout::GENERAL)
                .new_layout(vk::ImageLayout::GENERAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(*image)
                .subresource_range(image_range)
        })
        .collect::<Vec<_>>();
    let flow_buffer_barrier = [vk::BufferMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::NONE)
        .dst_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
        .dst_access_mask(vk::AccessFlags2::SHADER_STORAGE_READ)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .buffer(flow_vectors.buffer)
        .offset(0)
        .size(flow_vectors.size)];
    let output_initial_barrier = vk::ImageMemoryBarrier2::default()
        .src_stage_mask(if output_initialized {
            vk::PipelineStageFlags2::COMPUTE_SHADER
        } else {
            vk::PipelineStageFlags2::NONE
        })
        .src_access_mask(if output_initialized {
            vk::AccessFlags2::SHADER_STORAGE_WRITE
        } else {
            vk::AccessFlags2::empty()
        })
        .dst_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
        .dst_access_mask(vk::AccessFlags2::SHADER_STORAGE_WRITE)
        .old_layout(if output_initialized {
            vk::ImageLayout::GENERAL
        } else {
            vk::ImageLayout::UNDEFINED
        })
        .new_layout(vk::ImageLayout::GENERAL)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(output_image)
        .subresource_range(image_range);
    let mut initial_barriers = input_barriers;
    initial_barriers.push(output_initial_barrier);
    let initial_dependency = vk::DependencyInfo::default()
        .image_memory_barriers(&initial_barriers)
        .buffer_memory_barriers(&flow_buffer_barrier);
    unsafe { device.cmd_pipeline_barrier2(command_buffer, &initial_dependency) };
    unsafe {
        device.cmd_bind_pipeline(
            command_buffer,
            vk::PipelineBindPoint::COMPUTE,
            resources.pipeline,
        );
        device.cmd_bind_descriptor_sets(
            command_buffer,
            vk::PipelineBindPoint::COMPUTE,
            resources.pipeline_layout,
            0,
            &[descriptor_set],
            &[],
        );
        device.cmd_dispatch(
            command_buffer,
            dimensions.width.div_ceil(8),
            dimensions.height.div_ceil(8),
            1,
        );
    }
    let output_barrier = vk::ImageMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
        .src_access_mask(vk::AccessFlags2::SHADER_STORAGE_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
        .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
        .old_layout(vk::ImageLayout::GENERAL)
        .new_layout(vk::ImageLayout::GENERAL)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(output_image)
        .subresource_range(image_range);
    let output_barriers = [output_barrier];
    let output_dependency = vk::DependencyInfo::default().image_memory_barriers(&output_barriers);
    unsafe { device.cmd_pipeline_barrier2(command_buffer, &output_dependency) };
    let copy_region = [vk::BufferImageCopy::default()
        .image_subresource(vk::ImageSubresourceLayers {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            mip_level: 0,
            base_array_layer: 0,
            layer_count: 1,
        })
        .image_extent(vk::Extent3D {
            width: dimensions.width,
            height: dimensions.height,
            depth: 1,
        })];
    unsafe {
        device.cmd_copy_image_to_buffer(
            command_buffer,
            output_image,
            vk::ImageLayout::GENERAL,
            readback_buffer.buffer,
            &copy_region,
        );
        device.end_command_buffer(command_buffer)
    }
    .map_err(|error| format!("cannot finish interpolation command buffer: {error:?}"))?;
    resources.fence = unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) }
        .map_err(|error| format!("cannot create interpolation fence: {error:?}"))?;
    let command_buffers = [command_buffer];
    let wait_semaphores = [flow_complete];
    let wait_stages = [vk::PipelineStageFlags::COMPUTE_SHADER];
    let submit_info = [vk::SubmitInfo::default()
        .wait_semaphores(&wait_semaphores)
        .wait_dst_stage_mask(&wait_stages)
        .command_buffers(&command_buffers)];
    unsafe { device.queue_submit(queue, &submit_info, resources.fence) }
        .map_err(|error| format!("cannot submit interpolation compute work: {error:?}"))?;
    unsafe { device.wait_for_fences(&[resources.fence], true, u64::MAX) }
        .map_err(|error| format!("cannot wait for interpolation compute work: {error:?}"))?;
    read_staging_buffer(device, readback_buffer)
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(feature = "nvidia-optical-flow")]
pub struct OpticalFlowSessionInfo {
    pub image_format: vk::Format,
    pub flow_vector_format: vk::Format,
    pub width: u32,
    pub height: u32,
}

#[cfg(feature = "nvidia-optical-flow")]
pub fn enumerate_adapters() -> Result<Vec<AdapterInfo>, String> {
    let entry = load_entry()?;
    let instance = create_instance(&entry)?;
    let result = enumerate_instance_adapters(&instance);
    unsafe { instance.destroy_instance(None) };
    result
}

fn load_entry() -> Result<Entry, String> {
    unsafe { Entry::load() }.map_err(|error| format!("cannot load Vulkan loader: {error}"))
}

#[cfg(feature = "nvidia-optical-flow")]
fn create_instance(entry: &Entry) -> Result<ash::Instance, String> {
    let app_name = c"livenyan";
    let app_info = vk::ApplicationInfo::default()
        .application_name(app_name)
        .engine_name(app_name)
        .api_version(vk::API_VERSION_1_2);
    let create_info = vk::InstanceCreateInfo::default().application_info(&app_info);
    unsafe { entry.create_instance(&create_info, None) }
        .map_err(|error| format!("cannot create Vulkan instance: {error:?}"))
}

#[cfg(feature = "nvidia-optical-flow")]
fn enumerate_instance_adapters(instance: &ash::Instance) -> Result<Vec<AdapterInfo>, String> {
    let devices = unsafe { instance.enumerate_physical_devices() }
        .map_err(|error| format!("cannot enumerate Vulkan adapters: {error:?}"))?;
    let mut adapters = Vec::new();
    for device in devices {
        let properties = unsafe { instance.get_physical_device_properties(device) };
        let queues = unsafe { instance.get_physical_device_queue_family_properties(device) };
        let Some(graphics_queue_family) = queues
            .iter()
            .position(|queue| {
                queue
                    .queue_flags
                    .contains(vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE)
            })
            .map(|index| index as u32)
        else {
            continue;
        };
        let extensions = unsafe { instance.enumerate_device_extension_properties(device) }
            .map_err(|error| format!("cannot query device extensions: {error:?}"))?;
        let nvidia_optical_flow = has_optical_flow_extension(&extensions);
        let optical_flow_queue_family = queues
            .iter()
            .position(|queue| queue.queue_flags.contains(vk::QueueFlags::OPTICAL_FLOW_NV))
            .map(|index| index as u32);
        adapters.push(AdapterInfo {
            name: unsafe {
                CStr::from_ptr(properties.device_name.as_ptr())
                    .to_string_lossy()
                    .into_owned()
            },
            device_type: properties.device_type,
            graphics_queue_family,
            optical_flow_queue_family,
            nvidia_optical_flow,
        });
    }
    Ok(adapters)
}

#[cfg(feature = "nvidia-optical-flow")]
fn has_optical_flow_extension(extensions: &[vk::ExtensionProperties]) -> bool {
    extensions.iter().any(|extension| {
        let name = unsafe { CStr::from_ptr(extension.extension_name.as_ptr()) };
        name == vk::NV_OPTICAL_FLOW_NAME
    })
}

impl SdlVulkanContext {
    fn collect_retired_scene_textures(&mut self) -> Result<(), String> {
        let mut index = 0;
        while index < self.retired_scene_textures.len() {
            let retired = &self.retired_scene_textures[index];
            let mut complete = true;
            for fence in &retired.fences {
                complete &= unsafe { self.device.get_fence_status(*fence) }.map_err(|error| {
                    format!("cannot query retired scene texture completion: {error:?}")
                })?;
            }
            if !complete {
                index += 1;
                continue;
            }
            let retired = self.retired_scene_textures.swap_remove(index);
            unsafe {
                destroy_vulkan_image(&self.device, retired.image);
                for upload in retired.uploads {
                    self.device.destroy_buffer(upload.staging.buffer, None);
                    self.device.free_memory(upload.staging.memory, None);
                    self.device.destroy_command_pool(upload.command_pool, None);
                    self.device.destroy_fence(upload.fence, None);
                }
            }
        }
        Ok(())
    }

    fn collect_completed_texture_uploads(&mut self) -> Result<(), String> {
        let mut index = 0;
        while index < self.pending_texture_uploads.len() {
            let pending = &self.pending_texture_uploads[index];
            let complete = unsafe { self.device.get_fence_status(pending.fence) }
                .map_err(|error| format!("cannot query texture upload completion: {error:?}"))?;
            if !complete {
                index += 1;
                continue;
            }
            let pending = self.pending_texture_uploads.swap_remove(index);
            unsafe {
                self.device.destroy_buffer(pending.staging.buffer, None);
                self.device.free_memory(pending.staging.memory, None);
                self.device.destroy_command_pool(pending.command_pool, None);
                self.device.destroy_fence(pending.fence, None);
            }
        }
        Ok(())
    }

    fn retire_scene_texture(&mut self, texture: crate::renderer::TextureId, image: VulkanImage) {
        let mut fences = Vec::with_capacity(3);
        if let Some(frame) = &self.scene_frame {
            fences.push(frame.fence);
        }
        if self.frame_fence != vk::Fence::null() {
            fences.push(self.frame_fence);
        }
        let mut uploads = Vec::new();
        while let Some(index) = self
            .pending_texture_uploads
            .iter()
            .position(|pending| pending.texture == texture)
        {
            let upload = self.pending_texture_uploads.swap_remove(index);
            fences.push(upload.fence);
            uploads.push(upload);
        }
        fences.sort_by_key(|fence| fence.as_raw());
        fences.dedup();
        self.retired_scene_textures.push(RetiredSceneTexture {
            image,
            fences,
            uploads,
        });
    }

    pub fn upload_scene_texture(
        &mut self,
        image: &crate::renderer::RgbaImage,
    ) -> Result<crate::renderer::TextureId, String> {
        self.collect_retired_scene_textures()?;
        self.collect_completed_texture_uploads()?;
        if image.w == 0 || image.h == 0 {
            return Err("scene texture dimensions must be nonzero".into());
        }
        let expected_size = u64::from(image.w)
            .checked_mul(u64::from(image.h))
            .and_then(|size| size.checked_mul(4))
            .ok_or_else(|| "scene texture dimensions are too large".to_string())?;
        if image.rgba.len() as u64 != expected_size {
            return Err(format!(
                "scene texture has {} bytes, expected {expected_size}",
                image.rgba.len()
            ));
        }
        let id = crate::renderer::TextureId(self.next_scene_texture_id);
        let next_id = self
            .next_scene_texture_id
            .checked_add(1)
            .ok_or_else(|| "scene texture ID space exhausted".to_string())?;
        let queue_families = [self.graphics_queue_family];
        let texture = create_vulkan_image(
            &self.instance,
            &self.device,
            self.physical_device,
            image.w,
            image.h,
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageUsageFlags::TRANSFER_DST
                | vk::ImageUsageFlags::TRANSFER_SRC
                | vk::ImageUsageFlags::SAMPLED,
            &queue_families,
        )?;
        let staging = match create_staging_buffer(
            &self.instance,
            &self.device,
            self.physical_device,
            expected_size,
            &[self.graphics_queue_family],
        ) {
            Ok(staging) => staging,
            Err(error) => {
                unsafe { destroy_vulkan_image(&self.device, texture) };
                return Err(error);
            }
        };
        if let Err(error) = write_staging_buffer(&self.device, &staging, &image.rgba) {
            unsafe {
                self.device.destroy_buffer(staging.buffer, None);
                self.device.free_memory(staging.memory, None);
                destroy_vulkan_image(&self.device, texture);
            }
            return Err(error);
        }
        let upload_result = enqueue_texture_upload(
            &self.device,
            self.graphics_queue,
            self.graphics_queue_family,
            texture.image,
            image.w,
            image.h,
            &staging,
            vk::ImageLayout::UNDEFINED,
        );
        let (command_pool, fence) = match upload_result {
            Ok(resources) => resources,
            Err(error) => {
                unsafe {
                    self.device.destroy_buffer(staging.buffer, None);
                    self.device.free_memory(staging.memory, None);
                    destroy_vulkan_image(&self.device, texture);
                }
                return Err(error);
            }
        };
        self.next_scene_texture_id = next_id;
        self.scene_textures.insert(
            id,
            SceneTexture {
                image: texture,
                width: image.w,
                height: image.h,
            },
        );
        self.pending_texture_uploads.push(PendingTextureUpload {
            texture: id,
            staging,
            command_pool,
            fence,
        });
        Ok(id)
    }

    pub fn update_scene_texture(
        &mut self,
        texture: crate::renderer::TextureId,
        image: &crate::renderer::RgbaImage,
    ) -> Result<(), String> {
        self.collect_retired_scene_textures()?;
        self.collect_completed_texture_uploads()?;
        if image.w == 0 || image.h == 0 {
            return Err("scene texture dimensions must be nonzero".into());
        }
        let expected_size = u64::from(image.w)
            .checked_mul(u64::from(image.h))
            .and_then(|size| size.checked_mul(4))
            .ok_or_else(|| "scene texture dimensions are too large".to_string())?;
        if image.rgba.len() as u64 != expected_size {
            return Err(format!(
                "scene texture has {} bytes, expected {expected_size}",
                image.rgba.len()
            ));
        }
        let resource = self
            .scene_textures
            .get(&texture)
            .ok_or_else(|| format!("unknown Vulkan scene texture {}", texture.0))?;
        if (resource.width, resource.height) != (image.w, image.h) {
            return Err("in-place Vulkan texture updates must preserve dimensions".into());
        }
        let image_handle = resource.image.image;
        let staging = create_staging_buffer(
            &self.instance,
            &self.device,
            self.physical_device,
            expected_size,
            &[self.graphics_queue_family],
        )?;
        if let Err(error) = write_staging_buffer(&self.device, &staging, &image.rgba) {
            unsafe {
                self.device.destroy_buffer(staging.buffer, None);
                self.device.free_memory(staging.memory, None);
            }
            return Err(error);
        }
        let upload = enqueue_texture_upload(
            &self.device,
            self.graphics_queue,
            self.graphics_queue_family,
            image_handle,
            image.w,
            image.h,
            &staging,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        );
        let (command_pool, fence) = match upload {
            Ok(resources) => resources,
            Err(error) => {
                unsafe {
                    self.device.destroy_buffer(staging.buffer, None);
                    self.device.free_memory(staging.memory, None);
                }
                return Err(error);
            }
        };
        self.pending_texture_uploads.push(PendingTextureUpload {
            texture,
            staging,
            command_pool,
            fence,
        });
        if self.scene_output == Some(texture) {
            self.scene_output = None;
        }
        Ok(())
    }

    pub fn destroy_scene_texture(
        &mut self,
        texture: crate::renderer::TextureId,
    ) -> Result<(), String> {
        if self.scene_output == Some(texture) {
            self.scene_output = None;
        }
        let Some(resource) = self.scene_textures.remove(&texture) else {
            return Err(format!("unknown Vulkan scene texture {}", texture.0));
        };
        self.retire_scene_texture(texture, resource.image);
        self.collect_retired_scene_textures()?;
        self.collect_completed_texture_uploads()?;
        Ok(())
    }

    pub fn scene_texture_dimensions(
        &self,
        texture: crate::renderer::TextureId,
    ) -> Option<(u32, u32)> {
        self.scene_textures
            .get(&texture)
            .map(|resource| (resource.width, resource.height))
    }

    pub fn compose_scene_offscreen(
        &mut self,
        draw_list: &crate::renderer::DrawList,
    ) -> Result<crate::renderer::TextureId, String> {
        self.collect_retired_scene_textures()?;
        self.collect_completed_texture_uploads()?;
        let (width, height) = draw_list.dimensions();
        if draw_list.commands().is_empty() {
            return Err("cannot compose an empty scene draw list".into());
        }
        if !matches!(
            draw_list.commands().first(),
            Some(crate::renderer::DrawCommand::Clear(_))
        ) {
            return Err("scene draw list must begin with a clear command".into());
        }
        let mut commands = Vec::new();
        for command in draw_list.commands() {
            match command {
                crate::renderer::DrawCommand::StrokeRect { rect, color, blend } => {
                    let horizontal = rect.height.min(1.0);
                    let vertical = rect.width.min(1.0);
                    commands.push(crate::renderer::DrawCommand::FillRect {
                        rect: crate::renderer::Rect {
                            x: rect.x,
                            y: rect.y,
                            width: rect.width,
                            height: horizontal,
                        },
                        color: *color,
                        blend: *blend,
                    });
                    if rect.height > horizontal {
                        commands.push(crate::renderer::DrawCommand::FillRect {
                            rect: crate::renderer::Rect {
                                x: rect.x,
                                y: rect.y + rect.height - horizontal,
                                width: rect.width,
                                height: horizontal,
                            },
                            color: *color,
                            blend: *blend,
                        });
                    }
                    if rect.height > 2.0 * horizontal && rect.width > vertical {
                        let middle_height = rect.height - 2.0 * horizontal;
                        commands.push(crate::renderer::DrawCommand::FillRect {
                            rect: crate::renderer::Rect {
                                x: rect.x,
                                y: rect.y + horizontal,
                                width: vertical,
                                height: middle_height,
                            },
                            color: *color,
                            blend: *blend,
                        });
                        commands.push(crate::renderer::DrawCommand::FillRect {
                            rect: crate::renderer::Rect {
                                x: rect.x + rect.width - vertical,
                                y: rect.y + horizontal,
                                width: vertical,
                                height: middle_height,
                            },
                            color: *color,
                            blend: *blend,
                        });
                    }
                }
                _ => commands.push(command.clone()),
            }
        }
        let copy_count = commands
            .iter()
            .filter(|command| matches!(command, crate::renderer::DrawCommand::Copy { .. }))
            .count();
        let fallback = if copy_count == 0 {
            match self.scene_fallback {
                Some(id) if self.scene_textures.contains_key(&id) => Some(id),
                _ => {
                    let id = self.upload_scene_texture(&crate::renderer::RgbaImage::new(
                        1,
                        1,
                        vec![255, 255, 255, 255],
                    )?)?;
                    self.scene_fallback = Some(id);
                    Some(id)
                }
            }
        } else {
            None
        };
        let fallback_id = fallback.or_else(|| self.scene_textures.keys().next().copied());
        let can_reuse_output = self.scene_output.is_some_and(|id| {
            !commands.iter().any(|command| {
                matches!(command, crate::renderer::DrawCommand::Copy { texture, .. } if *texture == id)
            }) && self.scene_textures.get(&id).is_some_and(|texture| {
                texture.width == width && texture.height == height
            })
        });
        let (output_id, target_image, target_view, new_target) = if can_reuse_output {
            let output_id = self.scene_output.expect("checked cached output");
            let target = self.scene_textures.get(&output_id).expect("cached target");
            (output_id, target.image.image, target.image.view, None)
        } else {
            let output_id = crate::renderer::TextureId(self.next_scene_texture_id);
            let next_id = self
                .next_scene_texture_id
                .checked_add(1)
                .ok_or_else(|| "scene texture ID space exhausted".to_string())?;
            let target = match create_vulkan_image(
                &self.instance,
                &self.device,
                self.physical_device,
                width,
                height,
                vk::Format::R8G8B8A8_UNORM,
                vk::ImageUsageFlags::STORAGE
                    | vk::ImageUsageFlags::TRANSFER_SRC
                    | vk::ImageUsageFlags::SAMPLED,
                &[self.graphics_queue_family],
            ) {
                Ok(target) => target,
                Err(error) => {
                    return Err(error);
                }
            };
            (
                output_id,
                target.image,
                target.view,
                Some((target, next_id)),
            )
        };
        let result = (|| {
            if self.scene_pipeline.is_none() {
                self.scene_pipeline = Some(create_scene_pipeline(&self.device)?);
            }
            if self.scene_frame.is_none() {
                self.scene_frame = Some(SceneFrameResources::new(
                    &self.device,
                    self.graphics_queue_family,
                )?);
            }
            let set_count = u32::try_from(commands.len())
                .map_err(|_| "scene command count exceeds Vulkan descriptor capacity")?;
            let frame = self.scene_frame.as_mut().expect("created scene resources");
            unsafe {
                self.device
                    .wait_for_fences(&[frame.fence], true, u64::MAX)
                    .map_err(|error| {
                        format!("cannot wait for prior scene composition: {error:?}")
                    })?;
            }
            frame.ensure_descriptor_capacity(&self.device, set_count)?;
            unsafe {
                self.device
                    .reset_command_buffer(
                        frame.command_buffer,
                        vk::CommandBufferResetFlags::empty(),
                    )
                    .map_err(|error| {
                        format!("cannot reset scene composition commands: {error:?}")
                    })?;
                if frame.descriptor_pool != vk::DescriptorPool::null() {
                    self.device
                        .reset_descriptor_pool(
                            frame.descriptor_pool,
                            vk::DescriptorPoolResetFlags::empty(),
                        )
                        .map_err(|error| format!("cannot reset scene descriptors: {error:?}"))?;
                }
            }
            let command_buffer = frame.command_buffer;
            let descriptor_pool = frame.descriptor_pool;
            let fence = frame.fence;
            let scene_pipeline = self
                .scene_pipeline
                .as_ref()
                .expect("created scene pipeline");
            let descriptor_set_layout = scene_pipeline.descriptor_set_layout;
            let pipeline_layout = scene_pipeline.pipeline_layout;
            let pipeline = scene_pipeline.pipeline;
            let samplers = scene_pipeline.samplers;
            let range = vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            };
            let layouts = [descriptor_set_layout];
            unsafe {
                self.device
                    .begin_command_buffer(command_buffer, &vk::CommandBufferBeginInfo::default())
                    .map_err(|error| format!("cannot begin scene command buffer: {error:?}"))?;
                let initial = [vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(if can_reuse_output {
                        vk::PipelineStageFlags2::COMPUTE_SHADER
                            | vk::PipelineStageFlags2::FRAGMENT_SHADER
                    } else {
                        vk::PipelineStageFlags2::NONE
                    })
                    .src_access_mask(if can_reuse_output {
                        vk::AccessFlags2::SHADER_SAMPLED_READ
                    } else {
                        vk::AccessFlags2::empty()
                    })
                    .old_layout(if can_reuse_output {
                        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
                    } else {
                        vk::ImageLayout::UNDEFINED
                    })
                    .new_layout(vk::ImageLayout::GENERAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(target_image)
                    .subresource_range(range)];
                self.device.cmd_pipeline_barrier2(
                    command_buffer,
                    &vk::DependencyInfo::default().image_memory_barriers(&initial),
                );
                self.device.cmd_bind_pipeline(
                    command_buffer,
                    vk::PipelineBindPoint::COMPUTE,
                    pipeline,
                );
            }

            for command in &commands {
                use crate::renderer::{BlendMode, DrawCommand, TextureFilter};
                let (texture, destination, source_uv, color, flags, filter) = match command {
                    DrawCommand::Clear(crate::renderer::Color(color)) => (
                        fallback_id.ok_or("scene needs a texture descriptor for clear command")?,
                        [0.0, 0.0, width as f32, height as f32],
                        [0.0, 0.0, 1.0, 1.0],
                        color.map(|channel| channel as f32 / 255.0),
                        4,
                        TextureFilter::Nearest,
                    ),
                    DrawCommand::Copy {
                        texture,
                        source,
                        destination,
                        alpha,
                        filter,
                        blend,
                    } => {
                        let resource = self
                            .scene_textures
                            .get(texture)
                            .ok_or_else(|| format!("unknown Vulkan scene texture {}", texture.0))?;
                        let source = source.unwrap_or(crate::renderer::Rect {
                            x: 0.0,
                            y: 0.0,
                            width: resource.width as f32,
                            height: resource.height as f32,
                        });
                        (
                            *texture,
                            [
                                destination.x,
                                destination.y,
                                destination.width,
                                destination.height,
                            ],
                            [
                                source.x / resource.width as f32,
                                source.y / resource.height as f32,
                                source.width / resource.width as f32,
                                source.height / resource.height as f32,
                            ],
                            [1.0, 1.0, 1.0, *alpha as f32 / 255.0],
                            1 | if *blend == BlendMode::Alpha { 2 } else { 0 },
                            *filter,
                        )
                    }
                    DrawCommand::FillRect {
                        rect,
                        color: crate::renderer::Color(color),
                        blend,
                    } => (
                        fallback_id.ok_or("scene needs a texture descriptor for fill command")?,
                        [rect.x, rect.y, rect.width, rect.height],
                        [0.0, 0.0, 1.0, 1.0],
                        color.map(|channel| channel as f32 / 255.0),
                        if *blend == BlendMode::Alpha { 2 } else { 0 },
                        TextureFilter::Nearest,
                    ),
                    DrawCommand::StrokeRect { .. } => unreachable!(),
                };
                let dispatch_rect = if flags & 4 != 0 {
                    [0, 0, width, height]
                } else {
                    let start_x = (destination[0].floor() as i64).clamp(0, i64::from(width));
                    let start_y = (destination[1].floor() as i64).clamp(0, i64::from(height));
                    let end_x = ((destination[0] + destination[2]).ceil() as i64)
                        .clamp(0, i64::from(width));
                    let end_y = ((destination[1] + destination[3]).ceil() as i64)
                        .clamp(0, i64::from(height));
                    if end_x <= start_x || end_y <= start_y {
                        continue;
                    }
                    [
                        start_x as u32,
                        start_y as u32,
                        (end_x - start_x) as u32,
                        (end_y - start_y) as u32,
                    ]
                };
                let texture_resource = self
                    .scene_textures
                    .get(&texture)
                    .ok_or_else(|| format!("unknown Vulkan scene texture {}", texture.0))?;
                let descriptor_set = unsafe {
                    self.device.allocate_descriptor_sets(
                        &vk::DescriptorSetAllocateInfo::default()
                            .descriptor_pool(descriptor_pool)
                            .set_layouts(&layouts),
                    )
                }
                .map_err(|error| format!("cannot allocate scene descriptors: {error:?}"))?[0];
                let texture_info = [vk::DescriptorImageInfo::default()
                    .sampler(samplers[usize::from(filter == TextureFilter::Linear)])
                    .image_view(texture_resource.image.view)
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
                let target_info = [vk::DescriptorImageInfo::default()
                    .image_view(target_view)
                    .image_layout(vk::ImageLayout::GENERAL)];
                let writes = [
                    vk::WriteDescriptorSet::default()
                        .dst_set(descriptor_set)
                        .dst_binding(0)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .image_info(&texture_info),
                    vk::WriteDescriptorSet::default()
                        .dst_set(descriptor_set)
                        .dst_binding(1)
                        .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                        .image_info(&target_info),
                ];
                unsafe {
                    self.device.update_descriptor_sets(&writes, &[]);
                    self.device.cmd_bind_descriptor_sets(
                        command_buffer,
                        vk::PipelineBindPoint::COMPUTE,
                        pipeline_layout,
                        0,
                        &[descriptor_set],
                        &[],
                    );
                    let params = SceneDrawParameters {
                        destination,
                        target_size: [width as f32, height as f32],
                        dispatch_origin: [dispatch_rect[0] as f32, dispatch_rect[1] as f32],
                        source_uv,
                        color,
                        flags,
                    };
                    self.device.cmd_push_constants(
                        command_buffer,
                        pipeline_layout,
                        vk::ShaderStageFlags::COMPUTE,
                        0,
                        std::slice::from_raw_parts(
                            (&params as *const SceneDrawParameters).cast::<u8>(),
                            std::mem::size_of::<SceneDrawParameters>(),
                        ),
                    );
                    self.device.cmd_dispatch(
                        command_buffer,
                        dispatch_rect[2].div_ceil(8),
                        dispatch_rect[3].div_ceil(8),
                        1,
                    );
                    let barrier = [vk::ImageMemoryBarrier2::default()
                        .src_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
                        .src_access_mask(vk::AccessFlags2::SHADER_STORAGE_WRITE)
                        .dst_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
                        .dst_access_mask(
                            vk::AccessFlags2::SHADER_STORAGE_READ
                                | vk::AccessFlags2::SHADER_STORAGE_WRITE,
                        )
                        .old_layout(vk::ImageLayout::GENERAL)
                        .new_layout(vk::ImageLayout::GENERAL)
                        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                        .image(target_image)
                        .subresource_range(range)];
                    self.device.cmd_pipeline_barrier2(
                        command_buffer,
                        &vk::DependencyInfo::default().image_memory_barriers(&barrier),
                    );
                }
            }
            unsafe {
                let to_sample = [vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
                    .src_access_mask(vk::AccessFlags2::SHADER_STORAGE_WRITE)
                    .dst_stage_mask(
                        vk::PipelineStageFlags2::COMPUTE_SHADER
                            | vk::PipelineStageFlags2::FRAGMENT_SHADER,
                    )
                    .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
                    .old_layout(vk::ImageLayout::GENERAL)
                    .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(target_image)
                    .subresource_range(range)];
                self.device.cmd_pipeline_barrier2(
                    command_buffer,
                    &vk::DependencyInfo::default().image_memory_barriers(&to_sample),
                );
                self.device
                    .end_command_buffer(command_buffer)
                    .map_err(|error| format!("cannot finish scene command buffer: {error:?}"))?;
                self.device
                    .reset_fences(&[fence])
                    .map_err(|error| format!("cannot reset scene composition fence: {error:?}"))?;
                if let Err(error) = self.device.queue_submit(
                    self.graphics_queue,
                    &[vk::SubmitInfo::default().command_buffers(&[command_buffer])],
                    fence,
                ) {
                    let replacement = self
                        .device
                        .create_fence(
                            &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                            None,
                        )
                        .map_err(|fence_error| {
                            format!(
                                "cannot submit scene composition ({error:?}) or restore its fence ({fence_error:?})"
                            )
                        })?;
                    self.device.destroy_fence(fence, None);
                    self.scene_frame
                        .as_mut()
                        .expect("scene resources remain initialized")
                        .fence = replacement;
                    return Err(format!("cannot submit scene composition: {error:?}"));
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            if let Some((target, _)) = new_target {
                unsafe { destroy_vulkan_image(&self.device, target) };
            }
            return Err(error);
        }
        if let Some((target, next_id)) = new_target {
            self.next_scene_texture_id = next_id;
            self.scene_textures.insert(
                output_id,
                SceneTexture {
                    image: target,
                    width,
                    height,
                },
            );
        }
        self.scene_output = Some(output_id);
        Ok(output_id)
    }

    #[cfg(test)]
    pub fn read_scene_texture_for_test(
        &self,
        texture: crate::renderer::TextureId,
    ) -> Result<crate::renderer::RgbaImage, String> {
        let resource = self
            .scene_textures
            .get(&texture)
            .ok_or_else(|| format!("unknown Vulkan scene texture {}", texture.0))?;
        let width = resource.width;
        let height = resource.height;
        let size = u64::from(width) * u64::from(height) * 4;
        let readback = create_staging_buffer(
            &self.instance,
            &self.device,
            self.physical_device,
            size,
            &[self.graphics_queue_family],
        )?;
        let result = (|| {
            let pool = unsafe {
                self.device.create_command_pool(
                    &vk::CommandPoolCreateInfo::default()
                        .flags(vk::CommandPoolCreateFlags::TRANSIENT)
                        .queue_family_index(self.graphics_queue_family),
                    None,
                )
            }
            .map_err(|error| format!("cannot create scene readback pool: {error:?}"))?;
            let outcome = (|| {
                let command = unsafe {
                    self.device.allocate_command_buffers(
                        &vk::CommandBufferAllocateInfo::default()
                            .command_pool(pool)
                            .level(vk::CommandBufferLevel::PRIMARY)
                            .command_buffer_count(1),
                    )
                }
                .map_err(|error| format!("cannot allocate scene readback command: {error:?}"))?[0];
                unsafe {
                    self.device
                        .begin_command_buffer(command, &vk::CommandBufferBeginInfo::default())
                        .map_err(|error| format!("cannot begin scene readback: {error:?}"))?;
                    let range = vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: 0,
                        layer_count: 1,
                    };
                    let to_copy = [vk::ImageMemoryBarrier2::default()
                        .src_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
                        .src_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
                        .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                        .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
                        .old_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                        .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                        .image(resource.image.image)
                        .subresource_range(range)];
                    self.device.cmd_pipeline_barrier2(
                        command,
                        &vk::DependencyInfo::default().image_memory_barriers(&to_copy),
                    );
                    let regions = [vk::BufferImageCopy::default()
                        .image_subresource(vk::ImageSubresourceLayers {
                            aspect_mask: vk::ImageAspectFlags::COLOR,
                            mip_level: 0,
                            base_array_layer: 0,
                            layer_count: 1,
                        })
                        .image_extent(vk::Extent3D {
                            width,
                            height,
                            depth: 1,
                        })];
                    self.device.cmd_copy_image_to_buffer(
                        command,
                        resource.image.image,
                        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                        readback.buffer,
                        &regions,
                    );
                    let restore = [vk::ImageMemoryBarrier2::default()
                        .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                        .src_access_mask(vk::AccessFlags2::TRANSFER_READ)
                        .dst_stage_mask(
                            vk::PipelineStageFlags2::COMPUTE_SHADER
                                | vk::PipelineStageFlags2::FRAGMENT_SHADER,
                        )
                        .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
                        .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                        .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                        .image(resource.image.image)
                        .subresource_range(range)];
                    self.device.cmd_pipeline_barrier2(
                        command,
                        &vk::DependencyInfo::default().image_memory_barriers(&restore),
                    );
                    self.device
                        .end_command_buffer(command)
                        .map_err(|error| format!("cannot finish scene readback: {error:?}"))?;
                    submit_and_wait(&self.device, self.graphics_queue, command, "scene readback")?;
                }
                let pixels = read_staging_buffer(&self.device, &readback)?;
                crate::renderer::RgbaImage::new(width, height, pixels)
            })();
            unsafe { self.device.destroy_command_pool(pool, None) };
            outcome
        })();
        unsafe {
            self.device.destroy_buffer(readback.buffer, None);
            self.device.free_memory(readback.memory, None);
        }
        result
    }

    pub fn sdl_texture_image(&self, texture: &sdl3::render::Texture) -> Result<vk::Image, String> {
        let properties = unsafe { sdl3::sys::render::SDL_GetTextureProperties(texture.raw()) };
        if properties == 0
            || !unsafe {
                sdl3::sys::properties::SDL_HasProperty(
                    properties,
                    sdl3::sys::render::SDL_PROP_TEXTURE_VULKAN_TEXTURE_NUMBER,
                )
            }
        {
            return Err("SDL texture does not expose a Vulkan image".into());
        }
        let image = unsafe {
            sdl3::sys::properties::SDL_GetNumberProperty(
                properties,
                sdl3::sys::render::SDL_PROP_TEXTURE_VULKAN_TEXTURE_NUMBER,
                0,
            )
        };
        if image == 0 {
            return Err("SDL Vulkan texture image handle is null".into());
        }
        Ok(vk::Image::from_raw(image as u64))
    }

    pub fn create_sdl_renderer(
        &mut self,
        window: &sdl3::video::Window,
    ) -> Result<*mut sdl3::sys::render::SDL_Renderer, String> {
        self.release_native_swapchain()?;
        let properties = unsafe { sdl3::sys::properties::SDL_CreateProperties() };
        if properties == 0 {
            return Err(format!(
                "cannot create SDL Vulkan renderer properties: {}",
                sdl3::get_error()
            ));
        }
        let values = [
            unsafe {
                sdl3::sys::properties::SDL_SetStringProperty(
                    properties,
                    sdl3::sys::render::SDL_PROP_RENDERER_CREATE_NAME_STRING,
                    c"vulkan".as_ptr(),
                )
            },
            unsafe {
                sdl3::sys::properties::SDL_SetPointerProperty(
                    properties,
                    sdl3::sys::render::SDL_PROP_RENDERER_CREATE_WINDOW_POINTER,
                    window.raw().cast(),
                )
            },
            unsafe {
                sdl3::sys::properties::SDL_SetPointerProperty(
                    properties,
                    sdl3::sys::render::SDL_PROP_RENDERER_CREATE_VULKAN_INSTANCE_POINTER,
                    self.instance.handle().as_raw() as usize as *mut std::ffi::c_void,
                )
            },
            unsafe {
                sdl3::sys::properties::SDL_SetNumberProperty(
                    properties,
                    sdl3::sys::render::SDL_PROP_RENDERER_CREATE_VULKAN_SURFACE_NUMBER,
                    self.surface.as_raw() as i64,
                )
            },
            unsafe {
                sdl3::sys::properties::SDL_SetPointerProperty(
                    properties,
                    sdl3::sys::render::SDL_PROP_RENDERER_CREATE_VULKAN_PHYSICAL_DEVICE_POINTER,
                    self.physical_device.as_raw() as usize as *mut std::ffi::c_void,
                )
            },
            unsafe {
                sdl3::sys::properties::SDL_SetPointerProperty(
                    properties,
                    sdl3::sys::render::SDL_PROP_RENDERER_CREATE_VULKAN_DEVICE_POINTER,
                    self.device.handle().as_raw() as usize as *mut std::ffi::c_void,
                )
            },
            unsafe {
                sdl3::sys::properties::SDL_SetNumberProperty(
                    properties,
                    sdl3::sys::render::SDL_PROP_RENDERER_CREATE_VULKAN_GRAPHICS_QUEUE_FAMILY_INDEX_NUMBER,
                    i64::from(self.graphics_queue_family),
                )
            },
            unsafe {
                sdl3::sys::properties::SDL_SetNumberProperty(
                    properties,
                    sdl3::sys::render::SDL_PROP_RENDERER_CREATE_VULKAN_PRESENT_QUEUE_FAMILY_INDEX_NUMBER,
                    i64::from(self.graphics_queue_family),
                )
            },
        ];
        if values.into_iter().any(|success| !success) {
            unsafe { sdl3::sys::properties::SDL_DestroyProperties(properties) };
            return Err(format!(
                "cannot configure shared SDL Vulkan renderer: {}",
                sdl3::get_error()
            ));
        }
        let renderer = unsafe { sdl3::sys::render::SDL_CreateRendererWithProperties(properties) };
        unsafe { sdl3::sys::properties::SDL_DestroyProperties(properties) };
        if renderer.is_null() {
            return Err(format!(
                "cannot create SDL Vulkan renderer on shared Vulkan device: {}",
                sdl3::get_error()
            ));
        }
        Ok(renderer)
    }

    fn release_native_swapchain(&mut self) -> Result<(), String> {
        if !self.native_swapchain_active {
            return Ok(());
        }
        unsafe {
            self.device
                .device_wait_idle()
                .map_err(|error| format!("cannot wait before handing surface to SDL: {error:?}"))?;
        }
        self.collect_retired_scene_textures()?;
        self.collect_completed_texture_uploads()?;
        unsafe {
            self.device.destroy_fence(self.frame_fence, None);
            self.frame_fence = vk::Fence::null();
            for semaphore in self.render_finished.drain(..) {
                self.device.destroy_semaphore(semaphore, None);
            }
            self.device.destroy_semaphore(self.image_acquired, None);
            self.image_acquired = vk::Semaphore::null();
            self.device.destroy_command_pool(self.command_pool, None);
            self.command_pool = vk::CommandPool::null();
            self.command_buffer = vk::CommandBuffer::null();
            for framebuffer in self.framebuffers.drain(..) {
                self.device.destroy_framebuffer(framebuffer, None);
            }
            self.device.destroy_render_pass(self.render_pass, None);
            self.render_pass = vk::RenderPass::null();
            for view in self.swapchain_image_views.drain(..) {
                self.device.destroy_image_view(view, None);
            }
            self.swapchain_loader
                .destroy_swapchain(self.swapchain, None);
        }
        self.swapchain = vk::SwapchainKHR::null();
        self.swapchain_images.clear();
        self.native_swapchain_active = false;
        Ok(())
    }

    pub fn new(window: &sdl3::video::Window) -> Result<Self, String> {
        let entry = load_entry()?;
        let required_extensions = window
            .vulkan_instance_extensions()
            .map_err(|error| format!("cannot query SDL Vulkan extensions: {error}"))?;
        let extension_names = required_extensions
            .iter()
            .map(|extension| {
                std::ffi::CString::new(extension.as_str())
                    .map_err(|error| format!("invalid Vulkan extension name: {error}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let extension_pointers = extension_names
            .iter()
            .map(|extension| extension.as_ptr())
            .collect::<Vec<_>>();
        let app_name = c"livenyan";
        let app_info = vk::ApplicationInfo::default()
            .application_name(app_name)
            .engine_name(app_name)
            .api_version(vk::API_VERSION_1_2);
        let instance_info = vk::InstanceCreateInfo::default()
            .application_info(&app_info)
            .enabled_extension_names(&extension_pointers);
        let instance = unsafe { entry.create_instance(&instance_info, None) }
            .map_err(|error| format!("cannot create SDL Vulkan instance: {error:?}"))?;
        let surface = match unsafe { window.vulkan_create_surface(instance.handle()) } {
            Ok(surface) => surface,
            Err(error) => {
                unsafe { instance.destroy_instance(None) };
                return Err(format!("cannot create SDL Vulkan surface: {error}"));
            }
        };
        let surface_loader = ash::khr::surface::Instance::new(&entry, &instance);
        let devices = match unsafe { instance.enumerate_physical_devices() } {
            Ok(devices) => devices,
            Err(error) => {
                unsafe {
                    surface_loader.destroy_surface(surface, None);
                    instance.destroy_instance(None);
                }
                return Err(format!("cannot enumerate Vulkan adapters: {error:?}"));
            }
        };

        #[cfg(feature = "nvidia-optical-flow")]
        let candidate = devices.into_iter().find_map(|physical_device| {
            let extensions =
                unsafe { instance.enumerate_device_extension_properties(physical_device) }.ok()?;
            let has_swapchain = extensions.iter().any(|extension| unsafe {
                CStr::from_ptr(extension.extension_name.as_ptr()) == vk::KHR_SWAPCHAIN_NAME
            });
            if !has_swapchain || !has_optical_flow_extension(&extensions) {
                return None;
            }
            let has_synchronization2 = extensions.iter().any(|extension| unsafe {
                CStr::from_ptr(extension.extension_name.as_ptr()) == vk::KHR_SYNCHRONIZATION2_NAME
            });
            if !has_synchronization2 {
                return None;
            }
            let queues =
                unsafe { instance.get_physical_device_queue_family_properties(physical_device) };
            let graphics_queue_family = queues.iter().enumerate().find_map(|(index, queue)| {
                let supports_present = unsafe {
                    surface_loader.get_physical_device_surface_support(
                        physical_device,
                        index as u32,
                        surface,
                    )
                }
                .ok()?;
                (queue
                    .queue_flags
                    .contains(vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE)
                    && supports_present)
                    .then_some(index as u32)
            })?;
            let optical_flow_queue_family = queues.iter().position(|queue| {
                queue
                    .queue_flags
                    .contains(vk::QueueFlags::OPTICAL_FLOW_NV | vk::QueueFlags::TRANSFER)
            })? as u32;
            let mut optical_features = vk::PhysicalDeviceOpticalFlowFeaturesNV::default();
            let mut synchronization2_features =
                vk::PhysicalDeviceSynchronization2Features::default();
            let mut features = vk::PhysicalDeviceFeatures2::default()
                .push_next(&mut optical_features)
                .push_next(&mut synchronization2_features);
            unsafe { instance.get_physical_device_features2(physical_device, &mut features) };
            (optical_features.optical_flow == vk::TRUE
                && synchronization2_features.synchronization2 == vk::TRUE)
                .then_some((
                    physical_device,
                    graphics_queue_family,
                    optical_flow_queue_family,
                ))
        });
        #[cfg(not(feature = "nvidia-optical-flow"))]
        let candidate = devices.into_iter().find_map(|physical_device| {
            let extensions =
                unsafe { instance.enumerate_device_extension_properties(physical_device) }.ok()?;
            let has_swapchain = extensions.iter().any(|extension| unsafe {
                CStr::from_ptr(extension.extension_name.as_ptr()) == vk::KHR_SWAPCHAIN_NAME
            });
            let has_synchronization2 = extensions.iter().any(|extension| unsafe {
                CStr::from_ptr(extension.extension_name.as_ptr()) == vk::KHR_SYNCHRONIZATION2_NAME
            });
            if !has_swapchain || !has_synchronization2 {
                return None;
            }
            let queues =
                unsafe { instance.get_physical_device_queue_family_properties(physical_device) };
            let graphics_queue_family = queues.iter().enumerate().find_map(|(index, queue)| {
                let supports_present = unsafe {
                    surface_loader.get_physical_device_surface_support(
                        physical_device,
                        index as u32,
                        surface,
                    )
                }
                .ok()?;
                (queue
                    .queue_flags
                    .contains(vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE)
                    && supports_present)
                    .then_some(index as u32)
            })?;
            let mut synchronization2_features =
                vk::PhysicalDeviceSynchronization2Features::default();
            let mut features =
                vk::PhysicalDeviceFeatures2::default().push_next(&mut synchronization2_features);
            unsafe { instance.get_physical_device_features2(physical_device, &mut features) };
            (synchronization2_features.synchronization2 == vk::TRUE).then_some((
                physical_device,
                graphics_queue_family,
                graphics_queue_family,
            ))
        });
        let Some((physical_device, graphics_queue_family, _optical_flow_queue_family)) = candidate
        else {
            unsafe {
                surface_loader.destroy_surface(surface, None);
                instance.destroy_instance(None);
            }
            return Err("no adapter supports SDL Vulkan presentation and synchronization2".into());
        };
        #[cfg(feature = "nvidia-optical-flow")]
        let optical_flow_queue_family = _optical_flow_queue_family;

        let properties = unsafe { instance.get_physical_device_properties(physical_device) };
        let name = unsafe {
            CStr::from_ptr(properties.device_name.as_ptr())
                .to_string_lossy()
                .into_owned()
        };
        let priority = [1.0];
        let queue_infos = {
            #[allow(unused_mut)]
            let mut queue_infos = vec![
                vk::DeviceQueueCreateInfo::default()
                    .queue_family_index(graphics_queue_family)
                    .queue_priorities(&priority),
            ];
            #[cfg(feature = "nvidia-optical-flow")]
            if graphics_queue_family != optical_flow_queue_family {
                queue_infos.push(
                    vk::DeviceQueueCreateInfo::default()
                        .queue_family_index(optical_flow_queue_family)
                        .queue_priorities(&priority),
                );
            }
            queue_infos
        };
        #[allow(unused_mut)]
        let mut device_extensions = vec![
            vk::KHR_SWAPCHAIN_NAME.as_ptr(),
            vk::KHR_SYNCHRONIZATION2_NAME.as_ptr(),
        ];
        #[cfg(feature = "nvidia-optical-flow")]
        device_extensions.push(vk::NV_OPTICAL_FLOW_NAME.as_ptr());
        let mut synchronization2_features =
            vk::PhysicalDeviceSynchronization2Features::default().synchronization2(true);
        #[allow(unused_mut)]
        let mut device_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_infos)
            .enabled_extension_names(&device_extensions)
            .push_next(&mut synchronization2_features);
        #[cfg(feature = "nvidia-optical-flow")]
        let mut optical_features =
            vk::PhysicalDeviceOpticalFlowFeaturesNV::default().optical_flow(true);
        #[cfg(feature = "nvidia-optical-flow")]
        {
            device_info = device_info.push_next(&mut optical_features);
        }
        let device = match unsafe { instance.create_device(physical_device, &device_info, None) } {
            Ok(device) => device,
            Err(error) => {
                unsafe {
                    surface_loader.destroy_surface(surface, None);
                    instance.destroy_instance(None);
                }
                return Err(format!("cannot create SDL Vulkan device: {error:?}"));
            }
        };
        let graphics_queue = unsafe { device.get_device_queue(graphics_queue_family, 0) };
        let swapchain_loader = ash::khr::swapchain::Device::new(&instance, &device);
        #[cfg(feature = "nvidia-optical-flow")]
        let optical_flow_queue = unsafe { device.get_device_queue(optical_flow_queue_family, 0) };
        #[cfg(feature = "nvidia-optical-flow")]
        let optical_flow_instance = ash::nv::optical_flow::Instance::new(&entry, &instance);
        #[cfg(feature = "nvidia-optical-flow")]
        let optical_flow_device = ash::nv::optical_flow::Device::new(&instance, &device);
        let capabilities = match unsafe {
            surface_loader.get_physical_device_surface_capabilities(physical_device, surface)
        } {
            Ok(capabilities) => capabilities,
            Err(error) => {
                unsafe {
                    device.destroy_device(None);
                    surface_loader.destroy_surface(surface, None);
                    instance.destroy_instance(None);
                }
                return Err(format!(
                    "cannot query SDL Vulkan surface capabilities: {error:?}"
                ));
            }
        };
        if !capabilities
            .supported_usage_flags
            .contains(vk::ImageUsageFlags::TRANSFER_DST)
        {
            unsafe {
                device.destroy_device(None);
                surface_loader.destroy_surface(surface, None);
                instance.destroy_instance(None);
            }
            return Err(
                "SDL Vulkan surface does not support transfer-destination swapchain images".into(),
            );
        }
        let surface_formats = match unsafe {
            surface_loader.get_physical_device_surface_formats(physical_device, surface)
        } {
            Ok(formats) if !formats.is_empty() => formats,
            Ok(_) => {
                unsafe {
                    device.destroy_device(None);
                    surface_loader.destroy_surface(surface, None);
                    instance.destroy_instance(None);
                }
                return Err("SDL Vulkan surface exposes no presentation formats".into());
            }
            Err(error) => {
                unsafe {
                    device.destroy_device(None);
                    surface_loader.destroy_surface(surface, None);
                    instance.destroy_instance(None);
                }
                return Err(format!(
                    "cannot query SDL Vulkan surface formats: {error:?}"
                ));
            }
        };
        let surface_format = surface_formats
            .iter()
            .copied()
            .find(|format| {
                format.format == vk::Format::B8G8R8A8_SRGB
                    && format.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
            })
            .unwrap_or(surface_formats[0]);
        let (pixel_width, pixel_height) = window.size_in_pixels();
        let swapchain_extent = if capabilities.current_extent.width != u32::MAX {
            capabilities.current_extent
        } else {
            vk::Extent2D {
                width: pixel_width.clamp(
                    capabilities.min_image_extent.width,
                    capabilities.max_image_extent.width,
                ),
                height: pixel_height.clamp(
                    capabilities.min_image_extent.height,
                    capabilities.max_image_extent.height,
                ),
            }
        };
        let mut image_count = capabilities.min_image_count.saturating_add(1);
        if capabilities.max_image_count > 0 {
            image_count = image_count.min(capabilities.max_image_count);
        }
        let swapchain_queue_families = [graphics_queue_family];
        let swapchain_info = vk::SwapchainCreateInfoKHR::default()
            .surface(surface)
            .min_image_count(image_count)
            .image_format(surface_format.format)
            .image_color_space(surface_format.color_space)
            .image_extent(swapchain_extent)
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_DST)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .queue_family_indices(&swapchain_queue_families)
            .pre_transform(capabilities.current_transform)
            .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
            .present_mode(vk::PresentModeKHR::FIFO)
            .clipped(true);
        let swapchain = match unsafe { swapchain_loader.create_swapchain(&swapchain_info, None) } {
            Ok(swapchain) => swapchain,
            Err(error) => {
                unsafe {
                    device.destroy_device(None);
                    surface_loader.destroy_surface(surface, None);
                    instance.destroy_instance(None);
                }
                return Err(format!("cannot create SDL Vulkan swapchain: {error:?}"));
            }
        };
        let swapchain_images = match unsafe { swapchain_loader.get_swapchain_images(swapchain) } {
            Ok(images) => images,
            Err(error) => {
                unsafe {
                    swapchain_loader.destroy_swapchain(swapchain, None);
                    device.destroy_device(None);
                    surface_loader.destroy_surface(surface, None);
                    instance.destroy_instance(None);
                }
                return Err(format!(
                    "cannot retrieve SDL Vulkan swapchain images: {error:?}"
                ));
            }
        };
        let mut swapchain_image_views = Vec::with_capacity(swapchain_images.len());
        for image in &swapchain_images {
            let view_info = vk::ImageViewCreateInfo::default()
                .image(*image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(surface_format.format)
                .components(vk::ComponentMapping::default())
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                });
            match unsafe { device.create_image_view(&view_info, None) } {
                Ok(view) => swapchain_image_views.push(view),
                Err(error) => {
                    unsafe {
                        for view in swapchain_image_views {
                            device.destroy_image_view(view, None);
                        }
                        swapchain_loader.destroy_swapchain(swapchain, None);
                        device.destroy_device(None);
                        surface_loader.destroy_surface(surface, None);
                        instance.destroy_instance(None);
                    }
                    return Err(format!(
                        "cannot create SDL Vulkan swapchain image view: {error:?}"
                    ));
                }
            }
        }
        let attachment = [vk::AttachmentDescription::default()
            .format(surface_format.format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::PRESENT_SRC_KHR)];
        let color_attachment = [vk::AttachmentReference {
            attachment: 0,
            layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        }];
        let subpass = [vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color_attachment)];
        let render_pass_info = vk::RenderPassCreateInfo::default()
            .attachments(&attachment)
            .subpasses(&subpass);
        let render_pass = match unsafe { device.create_render_pass(&render_pass_info, None) } {
            Ok(render_pass) => render_pass,
            Err(error) => {
                unsafe {
                    for view in swapchain_image_views {
                        device.destroy_image_view(view, None);
                    }
                    swapchain_loader.destroy_swapchain(swapchain, None);
                    device.destroy_device(None);
                    surface_loader.destroy_surface(surface, None);
                    instance.destroy_instance(None);
                }
                return Err(format!("cannot create Vulkan clear render pass: {error:?}"));
            }
        };
        let mut framebuffers = Vec::with_capacity(swapchain_image_views.len());
        for view in &swapchain_image_views {
            let attachments = [*view];
            let framebuffer_info = vk::FramebufferCreateInfo::default()
                .render_pass(render_pass)
                .attachments(&attachments)
                .width(swapchain_extent.width)
                .height(swapchain_extent.height)
                .layers(1);
            match unsafe { device.create_framebuffer(&framebuffer_info, None) } {
                Ok(framebuffer) => framebuffers.push(framebuffer),
                Err(error) => {
                    unsafe {
                        for framebuffer in framebuffers {
                            device.destroy_framebuffer(framebuffer, None);
                        }
                        device.destroy_render_pass(render_pass, None);
                        for view in swapchain_image_views {
                            device.destroy_image_view(view, None);
                        }
                        swapchain_loader.destroy_swapchain(swapchain, None);
                        device.destroy_device(None);
                        surface_loader.destroy_surface(surface, None);
                        instance.destroy_instance(None);
                    }
                    return Err(format!(
                        "cannot create Vulkan swapchain framebuffer: {error:?}"
                    ));
                }
            }
        }
        let command_pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(graphics_queue_family)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        let command_pool = match unsafe { device.create_command_pool(&command_pool_info, None) } {
            Ok(pool) => pool,
            Err(error) => {
                unsafe {
                    destroy_frame_resources(&device, &framebuffers, render_pass);
                    for view in swapchain_image_views {
                        device.destroy_image_view(view, None);
                    }
                    swapchain_loader.destroy_swapchain(swapchain, None);
                    device.destroy_device(None);
                    surface_loader.destroy_surface(surface, None);
                    instance.destroy_instance(None);
                }
                return Err(format!(
                    "cannot create Vulkan frame command pool: {error:?}"
                ));
            }
        };
        let command_buffer_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let command_buffer = match unsafe { device.allocate_command_buffers(&command_buffer_info) }
        {
            Ok(buffers) => buffers[0],
            Err(error) => {
                unsafe {
                    device.destroy_command_pool(command_pool, None);
                    destroy_frame_resources(&device, &framebuffers, render_pass);
                    for view in swapchain_image_views {
                        device.destroy_image_view(view, None);
                    }
                    swapchain_loader.destroy_swapchain(swapchain, None);
                    device.destroy_device(None);
                    surface_loader.destroy_surface(surface, None);
                    instance.destroy_instance(None);
                }
                return Err(format!(
                    "cannot allocate Vulkan frame command buffer: {error:?}"
                ));
            }
        };
        let semaphore_info = vk::SemaphoreCreateInfo::default();
        let image_acquired = match unsafe { device.create_semaphore(&semaphore_info, None) } {
            Ok(semaphore) => semaphore,
            Err(error) => {
                unsafe {
                    device.destroy_command_pool(command_pool, None);
                    destroy_frame_resources(&device, &framebuffers, render_pass);
                    for view in swapchain_image_views {
                        device.destroy_image_view(view, None);
                    }
                    swapchain_loader.destroy_swapchain(swapchain, None);
                    device.destroy_device(None);
                    surface_loader.destroy_surface(surface, None);
                    instance.destroy_instance(None);
                }
                return Err(format!("cannot create Vulkan acquire semaphore: {error:?}"));
            }
        };
        let mut render_finished = Vec::with_capacity(swapchain_images.len());
        for _ in &swapchain_images {
            match unsafe { device.create_semaphore(&semaphore_info, None) } {
                Ok(semaphore) => render_finished.push(semaphore),
                Err(error) => {
                    unsafe {
                        for semaphore in render_finished {
                            device.destroy_semaphore(semaphore, None);
                        }
                        device.destroy_semaphore(image_acquired, None);
                        device.destroy_command_pool(command_pool, None);
                        destroy_frame_resources(&device, &framebuffers, render_pass);
                        for view in swapchain_image_views {
                            device.destroy_image_view(view, None);
                        }
                        swapchain_loader.destroy_swapchain(swapchain, None);
                        device.destroy_device(None);
                        surface_loader.destroy_surface(surface, None);
                        instance.destroy_instance(None);
                    }
                    return Err(format!("cannot create Vulkan render semaphore: {error:?}"));
                }
            }
        }
        let fence_info = vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED);
        let frame_fence = match unsafe { device.create_fence(&fence_info, None) } {
            Ok(fence) => fence,
            Err(error) => {
                unsafe {
                    for semaphore in render_finished {
                        device.destroy_semaphore(semaphore, None);
                    }
                    device.destroy_semaphore(image_acquired, None);
                    device.destroy_command_pool(command_pool, None);
                    destroy_frame_resources(&device, &framebuffers, render_pass);
                    for view in swapchain_image_views {
                        device.destroy_image_view(view, None);
                    }
                    swapchain_loader.destroy_swapchain(swapchain, None);
                    device.destroy_device(None);
                    surface_loader.destroy_surface(surface, None);
                    instance.destroy_instance(None);
                }
                return Err(format!("cannot create Vulkan frame fence: {error:?}"));
            }
        };
        Ok(Self {
            _entry: entry,
            instance,
            surface_loader,
            #[cfg(feature = "nvidia-optical-flow")]
            optical_flow_instance,
            swapchain_loader,
            #[cfg(feature = "nvidia-optical-flow")]
            optical_flow_device,
            device,
            surface,
            swapchain,
            swapchain_images,
            swapchain_image_views,
            framebuffers,
            render_pass,
            command_pool,
            command_buffer,
            image_acquired,
            render_finished,
            frame_fence,
            swapchain_format: surface_format.format,
            swapchain_extent,
            physical_device,
            graphics_queue,
            #[cfg(feature = "nvidia-optical-flow")]
            optical_flow_queue,
            graphics_queue_family,
            #[cfg(feature = "nvidia-optical-flow")]
            optical_flow_queue_family,
            name,
            #[cfg(feature = "nvidia-optical-flow")]
            optical_flow_session: None,
            #[cfg(feature = "nvidia-optical-flow")]
            optical_flow_session_info: None,
            #[cfg(feature = "nvidia-optical-flow")]
            optical_flow_images: Vec::new(),
            #[cfg(feature = "nvidia-optical-flow")]
            optical_flow_staging_buffers: Vec::new(),
            #[cfg(feature = "nvidia-optical-flow")]
            optical_flow_probe_initialized: false,
            scene_textures: HashMap::new(),
            next_scene_texture_id: 1,
            scene_output: None,
            scene_fallback: None,
            scene_pipeline: None,
            scene_frame: None,
            pending_texture_uploads: Vec::new(),
            retired_scene_textures: Vec::new(),
            native_swapchain_active: true,
            present_scale_filter: crate::scale_filter::ScaleFilter::Linear,
        })
    }

    #[cfg(feature = "nvidia-optical-flow")]
    pub fn optical_flow_input_formats(&self) -> Result<Vec<vk::Format>, String> {
        self.optical_flow_formats(vk::OpticalFlowUsageFlagsNV::INPUT)
    }

    #[cfg(feature = "nvidia-optical-flow")]
    fn optical_flow_formats(
        &self,
        usage: vk::OpticalFlowUsageFlagsNV,
    ) -> Result<Vec<vk::Format>, String> {
        let format_info = vk::OpticalFlowImageFormatInfoNV::default().usage(usage);
        let get_formats = self
            .optical_flow_instance
            .fp()
            .get_physical_device_optical_flow_image_formats_nv;
        let mut format_count = 0;
        let result = unsafe {
            get_formats(
                self.physical_device,
                &format_info,
                &mut format_count,
                std::ptr::null_mut(),
            )
        };
        if result != vk::Result::SUCCESS {
            return Err(format!(
                "cannot query NVIDIA optical-flow format count: {result:?}"
            ));
        }
        let mut properties =
            vec![vk::OpticalFlowImageFormatPropertiesNV::default(); format_count as usize];
        let result = unsafe {
            get_formats(
                self.physical_device,
                &format_info,
                &mut format_count,
                properties.as_mut_ptr(),
            )
        };
        if result != vk::Result::SUCCESS {
            return Err(format!(
                "cannot query NVIDIA optical-flow formats: {result:?}"
            ));
        }
        Ok(properties
            .into_iter()
            .take(format_count as usize)
            .map(|property| property.format)
            .collect())
    }

    #[cfg(feature = "nvidia-optical-flow")]
    pub fn create_optical_flow_session(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<OpticalFlowSessionInfo, String> {
        if width == 0 || height == 0 {
            return Err("optical-flow session dimensions must be nonzero".into());
        }
        if self.optical_flow_session.is_some() {
            return Err("an optical-flow session already exists for this renderer".into());
        }
        let supported_formats = self.optical_flow_input_formats()?;
        let image_format = [vk::Format::R8G8B8A8_UNORM, vk::Format::B8G8R8A8_UNORM]
        .into_iter()
        .find(|format| supported_formats.contains(format))
        .ok_or_else(|| {
            format!(
                "NVIDIA optical flow has no supported RGBA/BGRA input format (reported: {supported_formats:?})"
            )
        })?;
        let flow_vector_format = vk::Format::R16G16_S10_5_NV;
        let output_formats = self.optical_flow_formats(vk::OpticalFlowUsageFlagsNV::OUTPUT)?;
        if !output_formats.contains(&flow_vector_format) {
            return Err(format!(
                "NVIDIA optical flow does not support the requested vector format {flow_vector_format:?} (reported: {output_formats:?})"
            ));
        }
        let create_info = vk::OpticalFlowSessionCreateInfoNV::default()
            .width(width)
            .height(height)
            .image_format(image_format)
            .flow_vector_format(flow_vector_format)
            .cost_format(vk::Format::UNDEFINED)
            .output_grid_size(vk::OpticalFlowGridSizeFlagsNV::TYPE_4X4)
            .hint_grid_size(vk::OpticalFlowGridSizeFlagsNV::UNKNOWN)
            .performance_level(vk::OpticalFlowPerformanceLevelNV::FAST);
        let mut session = vk::OpticalFlowSessionNV::null();
        let result = unsafe {
            (self.optical_flow_device.fp().create_optical_flow_session_nv)(
                self.device.handle(),
                &create_info,
                std::ptr::null(),
                &mut session,
            )
        };
        if result != vk::Result::SUCCESS {
            return Err(format!(
                "cannot create NVIDIA optical-flow session: {result:?}"
            ));
        }
        self.optical_flow_session = Some(session);
        log::info!("NVIDIA Optical Flow input format: {image_format:?}");
        let session_info = OpticalFlowSessionInfo {
            image_format,
            flow_vector_format,
            width,
            height,
        };
        self.optical_flow_session_info = Some(session_info.clone());
        Ok(session_info)
    }

    #[cfg(feature = "nvidia-optical-flow")]
    pub fn ensure_optical_flow_session(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<OpticalFlowSessionInfo, String> {
        if let Some(session_info) = &self.optical_flow_session_info {
            if (session_info.width, session_info.height) != (width, height) {
                return Err(format!(
                    "Optical Flow session is {}x{}, requested {width}x{height}",
                    session_info.width, session_info.height
                ));
            }
            return Ok(session_info.clone());
        }
        self.create_optical_flow_session(width, height)
    }

    #[cfg(feature = "nvidia-optical-flow")]
    pub fn execute_optical_flow_frames(
        &mut self,
        previous_frame: &[u8],
        next_frame: &[u8],
    ) -> Result<(crate::renderer::RgbaImage, (f32, f32)), String> {
        let session = self
            .optical_flow_session
            .ok_or("create an optical-flow session before executing it")?;
        let session_info = self
            .optical_flow_session_info
            .as_ref()
            .cloned()
            .ok_or("optical-flow session metadata is unavailable")?;
        let input_size = u64::from(session_info.width)
            .checked_mul(u64::from(session_info.height))
            .and_then(|size| size.checked_mul(4))
            .ok_or("optical-flow frame dimensions are too large")?;
        let expected_size = usize::try_from(input_size)
            .map_err(|_| "optical-flow frame dimensions exceed addressable memory")?;
        if previous_frame.len() != expected_size || next_frame.len() != expected_size {
            return Err(format!(
                "optical-flow frames must each contain {expected_size} RGBA bytes for {}x{}",
                session_info.width, session_info.height
            ));
        }

        let queue_families = if self.optical_flow_queue_family == self.graphics_queue_family {
            vec![self.optical_flow_queue_family]
        } else {
            vec![self.optical_flow_queue_family, self.graphics_queue_family]
        };
        let output_width = session_info.width.div_ceil(4);
        let output_height = session_info.height.div_ceil(4);
        let output_size = u64::from(output_width) * u64::from(output_height) * 4;
        if self.optical_flow_images.is_empty() {
            let input_usage = vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED;
            for usage in [
                vk::OpticalFlowUsageFlagsNV::INPUT,
                vk::OpticalFlowUsageFlagsNV::INPUT,
                vk::OpticalFlowUsageFlagsNV::OUTPUT,
            ] {
                let (width, height, format, flags) = if usage == vk::OpticalFlowUsageFlagsNV::INPUT
                {
                    (
                        session_info.width,
                        session_info.height,
                        session_info.image_format,
                        input_usage,
                    )
                } else {
                    (
                        output_width,
                        output_height,
                        session_info.flow_vector_format,
                        vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_SRC,
                    )
                };
                let image = create_optical_flow_image(
                    &self.instance,
                    &self.device,
                    self.physical_device,
                    width,
                    height,
                    format,
                    usage,
                    flags,
                    &queue_families,
                )?;
                self.optical_flow_images.push(image);
            }
            let interpolated = create_vulkan_image(
                &self.instance,
                &self.device,
                self.physical_device,
                session_info.width,
                session_info.height,
                vk::Format::R8G8B8A8_UNORM,
                vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::TRANSFER_SRC,
                &[self.graphics_queue_family],
            )?;
            self.optical_flow_images.push(interpolated);
            for size in [input_size, input_size, output_size, input_size] {
                let buffer = create_staging_buffer(
                    &self.instance,
                    &self.device,
                    self.physical_device,
                    size,
                    &queue_families,
                )?;
                self.optical_flow_staging_buffers.push(buffer);
            }
            let image_views = self
                .optical_flow_images
                .iter()
                .map(|image| image.view)
                .collect::<Vec<_>>();
            let bindings = [
                (vk::OpticalFlowSessionBindingPointNV::INPUT, image_views[0]),
                (
                    vk::OpticalFlowSessionBindingPointNV::REFERENCE,
                    image_views[1],
                ),
                (
                    vk::OpticalFlowSessionBindingPointNV::FLOW_VECTOR,
                    image_views[2],
                ),
            ];
            for (binding, view) in bindings {
                let result = unsafe {
                    (self
                        .optical_flow_device
                        .fp()
                        .bind_optical_flow_session_image_nv)(
                        self.device.handle(),
                        session,
                        binding,
                        view,
                        vk::ImageLayout::GENERAL,
                    )
                };
                if result != vk::Result::SUCCESS {
                    return Err(format!(
                        "cannot bind NVIDIA optical-flow session image: {result:?}"
                    ));
                }
            }
        }
        if self.optical_flow_images.len() != 4 || self.optical_flow_staging_buffers.len() != 4 {
            return Err("NVIDIA Optical Flow resources were only partially initialized".into());
        }
        let image_handles = self
            .optical_flow_images
            .iter()
            .map(|image| image.image)
            .collect::<Vec<_>>();
        let image_views = self
            .optical_flow_images
            .iter()
            .map(|image| image.view)
            .collect::<Vec<_>>();
        let previous_pixels = optical_flow_input_pixels(previous_frame, session_info.image_format)?;
        let next_pixels = optical_flow_input_pixels(next_frame, session_info.image_format)?;
        write_staging_buffer(
            &self.device,
            &self.optical_flow_staging_buffers[0],
            &previous_pixels,
        )?;
        write_staging_buffer(
            &self.device,
            &self.optical_flow_staging_buffers[1],
            &next_pixels,
        )?;

        let command_pool_info =
            vk::CommandPoolCreateInfo::default().queue_family_index(self.optical_flow_queue_family);
        let command_pool = unsafe { self.device.create_command_pool(&command_pool_info, None) }
            .map_err(|error| format!("cannot create optical-flow command pool: {error:?}"))?;
        let command_buffer_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let command_buffer =
            match unsafe { self.device.allocate_command_buffers(&command_buffer_info) } {
                Ok(buffers) => buffers[0],
                Err(error) => {
                    unsafe { self.device.destroy_command_pool(command_pool, None) };
                    return Err(format!(
                        "cannot allocate optical-flow command buffer: {error:?}"
                    ));
                }
            };
        let flow_complete = match unsafe {
            self.device
                .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
        } {
            Ok(semaphore) => semaphore,
            Err(error) => {
                unsafe { self.device.destroy_command_pool(command_pool, None) };
                return Err(format!(
                    "cannot create optical-flow completion semaphore: {error:?}"
                ));
            }
        };
        let resources_initialized = self.optical_flow_probe_initialized;
        let run_result = (|| unsafe {
            let begin_info = vk::CommandBufferBeginInfo::default();
            self.device
                .begin_command_buffer(command_buffer, &begin_info)
                .map_err(|error| format!("cannot begin optical-flow command buffer: {error:?}"))?;

            let subresource_range = vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            };
            let initial_input_barriers = image_handles[..2]
                .iter()
                .map(|image| {
                    vk::ImageMemoryBarrier2::default()
                        .src_stage_mask(if resources_initialized {
                            vk::PipelineStageFlags2::OPTICAL_FLOW_NV
                        } else {
                            vk::PipelineStageFlags2::NONE
                        })
                        .src_access_mask(if resources_initialized {
                            vk::AccessFlags2::OPTICAL_FLOW_READ_NV
                        } else {
                            vk::AccessFlags2::empty()
                        })
                        .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                        .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                        .old_layout(if resources_initialized {
                            vk::ImageLayout::GENERAL
                        } else {
                            vk::ImageLayout::UNDEFINED
                        })
                        .new_layout(vk::ImageLayout::GENERAL)
                        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                        .image(*image)
                        .subresource_range(subresource_range)
                })
                .collect::<Vec<_>>();
            let input_barrier_info =
                vk::DependencyInfo::default().image_memory_barriers(&initial_input_barriers);
            self.device
                .cmd_pipeline_barrier2(command_buffer, &input_barrier_info);
            let input_copy_regions = [0, 1].map(|_| {
                vk::BufferImageCopy::default()
                    .image_subresource(vk::ImageSubresourceLayers {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        mip_level: 0,
                        base_array_layer: 0,
                        layer_count: 1,
                    })
                    .image_extent(vk::Extent3D {
                        width: session_info.width,
                        height: session_info.height,
                        depth: 1,
                    })
            });
            for index in 0..2 {
                self.device.cmd_copy_buffer_to_image(
                    command_buffer,
                    self.optical_flow_staging_buffers[index].buffer,
                    image_handles[index],
                    vk::ImageLayout::GENERAL,
                    &input_copy_regions[index..=index],
                );
            }
            let flow_input_barriers = image_handles[..2]
                .iter()
                .map(|image| {
                    vk::ImageMemoryBarrier2::default()
                        .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                        .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                        .dst_stage_mask(vk::PipelineStageFlags2::OPTICAL_FLOW_NV)
                        .dst_access_mask(vk::AccessFlags2::OPTICAL_FLOW_READ_NV)
                        .old_layout(vk::ImageLayout::GENERAL)
                        .new_layout(vk::ImageLayout::GENERAL)
                        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                        .image(*image)
                        .subresource_range(subresource_range)
                })
                .collect::<Vec<_>>();
            let output_initial_barrier = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(if resources_initialized {
                    vk::PipelineStageFlags2::OPTICAL_FLOW_NV
                } else {
                    vk::PipelineStageFlags2::NONE
                })
                .src_access_mask(if resources_initialized {
                    vk::AccessFlags2::OPTICAL_FLOW_WRITE_NV
                } else {
                    vk::AccessFlags2::empty()
                })
                .dst_stage_mask(vk::PipelineStageFlags2::OPTICAL_FLOW_NV)
                .dst_access_mask(vk::AccessFlags2::OPTICAL_FLOW_WRITE_NV)
                .old_layout(if resources_initialized {
                    vk::ImageLayout::GENERAL
                } else {
                    vk::ImageLayout::UNDEFINED
                })
                .new_layout(vk::ImageLayout::GENERAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image_handles[2])
                .subresource_range(subresource_range);
            let mut optical_flow_barriers = flow_input_barriers;
            optical_flow_barriers.push(output_initial_barrier);
            let optical_flow_barrier_info =
                vk::DependencyInfo::default().image_memory_barriers(&optical_flow_barriers);
            self.device
                .cmd_pipeline_barrier2(command_buffer, &optical_flow_barrier_info);
            let regions = [vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: vk::Extent2D {
                    width: session_info.width,
                    height: session_info.height,
                },
            }];
            let execute_info = vk::OpticalFlowExecuteInfoNV::default().regions(&regions);
            (self.optical_flow_device.fp().cmd_optical_flow_execute_nv)(
                command_buffer,
                session,
                &execute_info,
            );
            let output_barrier = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::OPTICAL_FLOW_NV)
                .src_access_mask(vk::AccessFlags2::OPTICAL_FLOW_WRITE_NV)
                .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
                .old_layout(vk::ImageLayout::GENERAL)
                .new_layout(vk::ImageLayout::GENERAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image_handles[2])
                .subresource_range(subresource_range);
            let output_barriers = [output_barrier];
            let output_barrier_info =
                vk::DependencyInfo::default().image_memory_barriers(&output_barriers);
            self.device
                .cmd_pipeline_barrier2(command_buffer, &output_barrier_info);
            let output_copy = [vk::BufferImageCopy::default()
                .image_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .image_extent(vk::Extent3D {
                    width: output_width,
                    height: output_height,
                    depth: 1,
                })];
            self.device.cmd_copy_image_to_buffer(
                command_buffer,
                image_handles[2],
                vk::ImageLayout::GENERAL,
                self.optical_flow_staging_buffers[2].buffer,
                &output_copy,
            );
            self.device
                .end_command_buffer(command_buffer)
                .map_err(|error| format!("cannot end optical-flow command buffer: {error:?}"))?;
            let command_buffers = [command_buffer];
            let signal_semaphores = [flow_complete];
            let submit_info = [vk::SubmitInfo::default()
                .command_buffers(&command_buffers)
                .signal_semaphores(&signal_semaphores)];
            let fence = self
                .device
                .create_fence(&vk::FenceCreateInfo::default(), None)
                .map_err(|error| format!("cannot create optical-flow fence: {error:?}"))?;
            let result = self
                .device
                .queue_submit(self.optical_flow_queue, &submit_info, fence)
                .map_err(|error| format!("cannot submit NVIDIA optical-flow work: {error:?}"))
                .and_then(|()| {
                    self.device
                        .wait_for_fences(&[fence], true, u64::MAX)
                        .map_err(|error| {
                            format!("cannot wait for NVIDIA optical-flow work: {error:?}")
                        })
                });
            self.device.destroy_fence(fence, None);
            result
        })();
        unsafe { self.device.destroy_command_pool(command_pool, None) };
        if let Err(error) = run_result {
            unsafe { self.device.destroy_semaphore(flow_complete, None) };
            return Err(error);
        }
        let probe_result = (|| {
            let vectors = read_staging_buffer(&self.device, &self.optical_flow_staging_buffers[2])?;
            let mean_flow = mean_probe_flow(&vectors, output_width, output_height)?;
            let pixels = interpolate_frames_on_gpu(
                &self.device,
                self.graphics_queue,
                self.graphics_queue_family,
                vk::Extent2D {
                    width: session_info.width,
                    height: session_info.height,
                },
                image_handles[..3].try_into().expect("three input images"),
                image_views[..2].try_into().expect("two source image views"),
                &self.optical_flow_staging_buffers[2],
                image_handles[3],
                image_views[3],
                &self.optical_flow_staging_buffers[3],
                flow_complete,
                resources_initialized,
            )?;
            let image =
                crate::renderer::RgbaImage::new(session_info.width, session_info.height, pixels)?;
            Ok((image, (mean_flow.0, mean_flow.1)))
        })();
        unsafe { self.device.destroy_semaphore(flow_complete, None) };
        if probe_result.is_ok() {
            self.optical_flow_probe_initialized = true;
        }
        probe_result
    }

    #[cfg(feature = "nvidia-optical-flow")]
    pub fn execute_optical_flow_probe(&mut self) -> Result<(f32, f32, u8), String> {
        let (width, height) = self
            .optical_flow_session_info
            .as_ref()
            .map(|info| (info.width, info.height))
            .ok_or("create an optical-flow session before executing it")?;
        let previous = make_translation_probe_frame(width, height, 0);
        let next = make_translation_probe_frame(width, height, 12);
        let (image, flow) = self.execute_optical_flow_frames(&previous, &next)?;
        let offset = ((height / 2 * width + width / 2) * 4) as usize;
        let luminance = *image
            .rgba
            .get(offset)
            .ok_or("interpolation probe pixel is outside its output image")?;
        Ok((flow.0, flow.1, luminance))
    }

    /// Filtro del blit de present (letterbox lógico → swapchain).
    pub fn set_present_scale_filter(
        &mut self,
        filter: crate::scale_filter::ScaleFilter,
    ) -> Result<(), String> {
        self.present_scale_filter = filter;
        Ok(())
    }

    fn present_blit_filter(&self) -> vk::Filter {
        if self.present_scale_filter.present_nearest() {
            vk::Filter::NEAREST
        } else {
            vk::Filter::LINEAR
        }
    }

    pub fn present_clear(&mut self, color: [f32; 4]) -> Result<(), String> {
        if !self.native_swapchain_active {
            return Err("native Vulkan swapchain was handed to the SDL renderer".into());
        }
        unsafe {
            self.device
                .wait_for_fences(&[self.frame_fence], true, u64::MAX)
                .map_err(|error| format!("cannot wait for Vulkan frame fence: {error:?}"))?;
        }
        let (image_index, _) = unsafe {
            self.swapchain_loader.acquire_next_image(
                self.swapchain,
                u64::MAX,
                self.image_acquired,
                vk::Fence::null(),
            )
        }
        .map_err(|error| format!("cannot acquire Vulkan swapchain image: {error:?}"))?;
        unsafe {
            self.device
                .reset_fences(&[self.frame_fence])
                .map_err(|error| format!("cannot reset Vulkan frame fence: {error:?}"))?;
            self.device
                .reset_command_buffer(self.command_buffer, vk::CommandBufferResetFlags::empty())
                .map_err(|error| format!("cannot reset Vulkan frame command buffer: {error:?}"))?;
            let begin_info = vk::CommandBufferBeginInfo::default();
            self.device
                .begin_command_buffer(self.command_buffer, &begin_info)
                .map_err(|error| format!("cannot begin Vulkan frame command buffer: {error:?}"))?;
            let clear_values = [vk::ClearValue {
                color: vk::ClearColorValue { float32: color },
            }];
            let render_info = vk::RenderPassBeginInfo::default()
                .render_pass(self.render_pass)
                .framebuffer(self.framebuffers[image_index as usize])
                .render_area(vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent: self.swapchain_extent,
                })
                .clear_values(&clear_values);
            self.device.cmd_begin_render_pass(
                self.command_buffer,
                &render_info,
                vk::SubpassContents::INLINE,
            );
            self.device.cmd_end_render_pass(self.command_buffer);
            self.device
                .end_command_buffer(self.command_buffer)
                .map_err(|error| format!("cannot end Vulkan frame command buffer: {error:?}"))?;
            let wait_semaphores = [self.image_acquired];
            let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
            let command_buffers = [self.command_buffer];
            let signal_semaphores = [self.render_finished[image_index as usize]];
            let submit_info = [vk::SubmitInfo::default()
                .wait_semaphores(&wait_semaphores)
                .wait_dst_stage_mask(&wait_stages)
                .command_buffers(&command_buffers)
                .signal_semaphores(&signal_semaphores)];
            self.device
                .queue_submit(self.graphics_queue, &submit_info, self.frame_fence)
                .map_err(|error| format!("cannot submit Vulkan frame: {error:?}"))?;
            let swapchains = [self.swapchain];
            let image_indices = [image_index];
            let present_info = vk::PresentInfoKHR::default()
                .wait_semaphores(&signal_semaphores)
                .swapchains(&swapchains)
                .image_indices(&image_indices);
            self.swapchain_loader
                .queue_present(self.graphics_queue, &present_info)
                .map_err(|error| format!("cannot present Vulkan frame: {error:?}"))?;
        }
        Ok(())
    }

    pub fn present_scene_texture(
        &mut self,
        texture: crate::renderer::TextureId,
    ) -> Result<(), String> {
        self.collect_retired_scene_textures()?;
        self.collect_completed_texture_uploads()?;
        if !self.native_swapchain_active {
            return Err("native Vulkan swapchain was handed to the SDL renderer".into());
        }
        let source = self
            .scene_textures
            .get(&texture)
            .ok_or_else(|| format!("unknown Vulkan scene texture {}", texture.0))?;
        let source_image = source.image.image;
        let source_extent = vk::Extent3D {
            width: source.width,
            height: source.height,
            depth: 1,
        };
        unsafe {
            self.device
                .wait_for_fences(&[self.frame_fence], true, u64::MAX)
                .map_err(|error| format!("cannot wait before Vulkan texture present: {error:?}"))?;
        }
        let (image_index, _) = unsafe {
            self.swapchain_loader.acquire_next_image(
                self.swapchain,
                u64::MAX,
                self.image_acquired,
                vk::Fence::null(),
            )
        }
        .map_err(|error| format!("cannot acquire Vulkan presentation image: {error:?}"))?;
        unsafe {
            self.device
                .reset_fences(&[self.frame_fence])
                .map_err(|error| format!("cannot reset Vulkan presentation fence: {error:?}"))?;
            self.device
                .reset_command_buffer(self.command_buffer, vk::CommandBufferResetFlags::empty())
                .map_err(|error| format!("cannot reset Vulkan presentation commands: {error:?}"))?;
            self.device
                .begin_command_buffer(self.command_buffer, &vk::CommandBufferBeginInfo::default())
                .map_err(|error| format!("cannot begin Vulkan texture presentation: {error:?}"))?;
            let range = vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            };
            let to_transfer = [
                vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(
                        vk::PipelineStageFlags2::COMPUTE_SHADER
                            | vk::PipelineStageFlags2::FRAGMENT_SHADER,
                    )
                    .src_access_mask(
                        vk::AccessFlags2::SHADER_SAMPLED_READ
                            | vk::AccessFlags2::SHADER_STORAGE_READ,
                    )
                    .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                    .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
                    .old_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(source_image)
                    .subresource_range(range),
                vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::NONE)
                    .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                    .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                    .old_layout(vk::ImageLayout::UNDEFINED)
                    .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(self.swapchain_images[image_index as usize])
                    .subresource_range(range),
            ];
            self.device.cmd_pipeline_barrier2(
                self.command_buffer,
                &vk::DependencyInfo::default().image_memory_barriers(&to_transfer),
            );
            self.device.cmd_clear_color_image(
                self.command_buffer,
                self.swapchain_images[image_index as usize],
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &vk::ClearColorValue {
                    float32: [0.0, 0.0, 0.0, 1.0],
                },
                &[range],
            );
            let (x, y, width, height) = crate::util::letterbox(
                (self.swapchain_extent.width, self.swapchain_extent.height),
                (source_extent.width, source_extent.height),
            );
            let blit = [vk::ImageBlit::default()
                .src_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .src_offsets([
                    vk::Offset3D { x: 0, y: 0, z: 0 },
                    vk::Offset3D {
                        x: source_extent.width as i32,
                        y: source_extent.height as i32,
                        z: 1,
                    },
                ])
                .dst_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .dst_offsets([
                    vk::Offset3D {
                        x: x.round() as i32,
                        y: y.round() as i32,
                        z: 0,
                    },
                    vk::Offset3D {
                        x: (x + width).round() as i32,
                        y: (y + height).round() as i32,
                        z: 1,
                    },
                ])];
            self.device.cmd_blit_image(
                self.command_buffer,
                source_image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                self.swapchain_images[image_index as usize],
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &blit,
                self.present_blit_filter(),
            );
            let to_present = [
                vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                    .src_access_mask(vk::AccessFlags2::TRANSFER_READ)
                    .dst_stage_mask(
                        vk::PipelineStageFlags2::COMPUTE_SHADER
                            | vk::PipelineStageFlags2::FRAGMENT_SHADER,
                    )
                    .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
                    .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(source_image)
                    .subresource_range(range),
                vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                    .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                    .dst_stage_mask(vk::PipelineStageFlags2::NONE)
                    .dst_access_mask(vk::AccessFlags2::empty())
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(self.swapchain_images[image_index as usize])
                    .subresource_range(range),
            ];
            self.device.cmd_pipeline_barrier2(
                self.command_buffer,
                &vk::DependencyInfo::default().image_memory_barriers(&to_present),
            );
            self.device
                .end_command_buffer(self.command_buffer)
                .map_err(|error| format!("cannot finish Vulkan texture presentation: {error:?}"))?;
            let wait_semaphores = [self.image_acquired];
            let wait_stages = [vk::PipelineStageFlags::TRANSFER];
            let command_buffers = [self.command_buffer];
            let signal_semaphores = [self.render_finished[image_index as usize]];
            let submit_info = [vk::SubmitInfo::default()
                .wait_semaphores(&wait_semaphores)
                .wait_dst_stage_mask(&wait_stages)
                .command_buffers(&command_buffers)
                .signal_semaphores(&signal_semaphores)];
            self.device
                .queue_submit(self.graphics_queue, &submit_info, self.frame_fence)
                .map_err(|error| format!("cannot submit Vulkan texture presentation: {error:?}"))?;
            let swapchains = [self.swapchain];
            let image_indices = [image_index];
            let present_info = vk::PresentInfoKHR::default()
                .wait_semaphores(&signal_semaphores)
                .swapchains(&swapchains)
                .image_indices(&image_indices);
            self.swapchain_loader
                .queue_present(self.graphics_queue, &present_info)
                .map_err(|error| format!("cannot present Vulkan scene texture: {error:?}"))?;
        }
        Ok(())
    }
}

impl crate::renderer::RendererBackend for SdlVulkanContext {
    fn upload_texture(
        &mut self,
        image: &crate::renderer::RgbaImage,
    ) -> Result<crate::renderer::TextureId, String> {
        self.upload_scene_texture(image)
    }

    fn update_texture(
        &mut self,
        texture: crate::renderer::TextureId,
        image: &crate::renderer::RgbaImage,
    ) -> Result<(), String> {
        if !self.scene_textures.contains_key(&texture) {
            return Err(format!("unknown Vulkan scene texture {}", texture.0));
        }
        if self.scene_texture_dimensions(texture) == Some((image.w, image.h)) {
            return self.update_scene_texture(texture, image);
        }
        let replacement_id = self.upload_scene_texture(image)?;
        let Some(replacement) = self.scene_textures.remove(&replacement_id) else {
            return Err("new Vulkan texture disappeared during update".into());
        };
        let Some(old) = self.scene_textures.remove(&texture) else {
            self.scene_textures.insert(replacement_id, replacement);
            return Err(format!("unknown Vulkan scene texture {}", texture.0));
        };
        self.scene_textures.insert(texture, replacement);
        self.retire_scene_texture(texture, old.image);
        for upload in &mut self.pending_texture_uploads {
            if upload.texture == replacement_id {
                upload.texture = texture;
            }
        }
        if self.scene_output == Some(texture) {
            self.scene_output = None;
        }
        self.collect_retired_scene_textures()?;
        self.collect_completed_texture_uploads()?;
        Ok(())
    }

    fn destroy_texture(&mut self, texture: crate::renderer::TextureId) -> Result<(), String> {
        self.destroy_scene_texture(texture)
    }

    fn submit(&mut self, draw_list: &crate::renderer::DrawList) -> Result<(), String> {
        let output = self.compose_scene_offscreen(draw_list)?;
        self.present_scene_texture(output)
    }
}

unsafe fn destroy_frame_resources(
    device: &ash::Device,
    framebuffers: &[vk::Framebuffer],
    render_pass: vk::RenderPass,
) {
    for framebuffer in framebuffers {
        unsafe { device.destroy_framebuffer(*framebuffer, None) };
    }
    unsafe { device.destroy_render_pass(render_pass, None) };
}

impl Drop for SdlVulkanContext {
    fn drop(&mut self) {
        if let Err(error) = unsafe { self.device.device_wait_idle() } {
            log::warn!("cannot wait for SDL Vulkan device: {error:?}");
        }
        unsafe {
            if let Some(pipeline) = self.scene_pipeline.take() {
                pipeline.destroy(&self.device);
            }
            if let Some(frame) = self.scene_frame.take() {
                frame.destroy(&self.device);
            }
            #[cfg(feature = "nvidia-optical-flow")]
            if let Some(session) = self.optical_flow_session.take() {
                (self
                    .optical_flow_device
                    .fp()
                    .destroy_optical_flow_session_nv)(
                    self.device.handle(),
                    session,
                    std::ptr::null(),
                );
            }
            #[cfg(feature = "nvidia-optical-flow")]
            for image in self.optical_flow_images.drain(..) {
                destroy_vulkan_image(&self.device, image);
            }
            #[cfg(feature = "nvidia-optical-flow")]
            for buffer in self.optical_flow_staging_buffers.drain(..) {
                self.device.destroy_buffer(buffer.buffer, None);
                self.device.free_memory(buffer.memory, None);
            }
            for (_, texture) in self.scene_textures.drain() {
                destroy_vulkan_image(&self.device, texture.image);
            }
            for upload in self.pending_texture_uploads.drain(..) {
                self.device.destroy_buffer(upload.staging.buffer, None);
                self.device.free_memory(upload.staging.memory, None);
                self.device.destroy_command_pool(upload.command_pool, None);
                self.device.destroy_fence(upload.fence, None);
            }
            for retired in self.retired_scene_textures.drain(..) {
                destroy_vulkan_image(&self.device, retired.image);
                for upload in retired.uploads {
                    self.device.destroy_buffer(upload.staging.buffer, None);
                    self.device.free_memory(upload.staging.memory, None);
                    self.device.destroy_command_pool(upload.command_pool, None);
                    self.device.destroy_fence(upload.fence, None);
                }
            }
            if self.frame_fence != vk::Fence::null() {
                self.device.destroy_fence(self.frame_fence, None);
            }
            for semaphore in self.render_finished.drain(..) {
                self.device.destroy_semaphore(semaphore, None);
            }
            if self.image_acquired != vk::Semaphore::null() {
                self.device.destroy_semaphore(self.image_acquired, None);
            }
            if self.command_pool != vk::CommandPool::null() {
                self.device.destroy_command_pool(self.command_pool, None);
            }
            for framebuffer in self.framebuffers.drain(..) {
                self.device.destroy_framebuffer(framebuffer, None);
            }
            if self.render_pass != vk::RenderPass::null() {
                self.device.destroy_render_pass(self.render_pass, None);
            }
            for view in self.swapchain_image_views.drain(..) {
                self.device.destroy_image_view(view, None);
            }
            if self.swapchain != vk::SwapchainKHR::null() {
                self.swapchain_loader
                    .destroy_swapchain(self.swapchain, None);
            }
            self.device.destroy_device(None);
            self.surface_loader.destroy_surface(self.surface, None);
            self.instance.destroy_instance(None);
        }
    }
}

#[cfg(feature = "nvidia-optical-flow")]
impl OpticalFlowDevice {
    pub fn new() -> Result<Self, String> {
        let entry = load_entry()?;
        let instance = create_instance(&entry)?;
        Self::create(entry, instance)
    }

    fn create(entry: Entry, instance: ash::Instance) -> Result<Self, String> {
        let devices = match unsafe { instance.enumerate_physical_devices() } {
            Ok(devices) => devices,
            Err(error) => {
                unsafe { instance.destroy_instance(None) };
                return Err(format!("cannot enumerate Vulkan adapters: {error:?}"));
            }
        };
        let candidate = devices.into_iter().find_map(|physical_device| {
            let extensions =
                unsafe { instance.enumerate_device_extension_properties(physical_device) }.ok()?;
            if !has_optical_flow_extension(&extensions) {
                return None;
            }
            let queue_family_index =
                unsafe { instance.get_physical_device_queue_family_properties(physical_device) }
                    .iter()
                    .position(|queue| {
                        queue
                            .queue_flags
                            .contains(vk::QueueFlags::OPTICAL_FLOW_NV | vk::QueueFlags::TRANSFER)
                    })? as u32;
            let mut optical_features = vk::PhysicalDeviceOpticalFlowFeaturesNV::default();
            let mut features =
                vk::PhysicalDeviceFeatures2::default().push_next(&mut optical_features);
            unsafe { instance.get_physical_device_features2(physical_device, &mut features) };
            (optical_features.optical_flow == vk::TRUE)
                .then_some((physical_device, queue_family_index))
        });
        let Some((physical_device, queue_family_index)) = candidate else {
            unsafe { instance.destroy_instance(None) };
            return Err(
                "no Vulkan adapter supports VK_NV_optical_flow and an optical-flow queue".into(),
            );
        };

        let properties = unsafe { instance.get_physical_device_properties(physical_device) };
        let name = unsafe {
            CStr::from_ptr(properties.device_name.as_ptr())
                .to_string_lossy()
                .into_owned()
        };
        let queue_priorities = [1.0];
        let queue_info = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family_index)
            .queue_priorities(&queue_priorities)];
        let mut optical_features =
            vk::PhysicalDeviceOpticalFlowFeaturesNV::default().optical_flow(true);
        let extension_names = [vk::NV_OPTICAL_FLOW_NAME.as_ptr()];
        let device_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_info)
            .enabled_extension_names(&extension_names)
            .push_next(&mut optical_features);
        let device = match unsafe { instance.create_device(physical_device, &device_info, None) } {
            Ok(device) => device,
            Err(error) => {
                unsafe { instance.destroy_instance(None) };
                return Err(format!(
                    "cannot create optical-flow Vulkan device: {error:?}"
                ));
            }
        };
        let queue = unsafe { device.get_device_queue(queue_family_index, 0) };
        let optical_flow = ash::nv::optical_flow::Device::new(&instance, &device);
        Ok(Self {
            _entry: entry,
            instance,
            device,
            optical_flow,
            physical_device,
            queue,
            queue_family_index,
            name,
        })
    }
}

#[cfg(feature = "nvidia-optical-flow")]
impl Drop for OpticalFlowDevice {
    fn drop(&mut self) {
        if let Err(error) = unsafe { self.device.device_wait_idle() } {
            log::warn!("cannot wait for optical-flow Vulkan device: {error:?}");
        }
        unsafe {
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SdlVulkanContext;
    #[cfg(feature = "nvidia-optical-flow")]
    use super::{OpticalFlowDevice, enumerate_adapters, optical_flow_input_pixels};
    use std::sync::Mutex;
    #[cfg(feature = "nvidia-optical-flow")]
    use std::time::Instant;

    static VULKAN_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    #[cfg(feature = "nvidia-optical-flow")]
    fn optical_flow_input_matches_selected_channel_order() {
        let rgba = [255, 32, 8, 255, 0, 128, 240, 255];
        assert_eq!(
            optical_flow_input_pixels(&rgba, ash::vk::Format::R8G8B8A8_UNORM)
                .unwrap()
                .as_ref(),
            rgba
        );
        assert_eq!(
            optical_flow_input_pixels(&rgba, ash::vk::Format::B8G8R8A8_UNORM)
                .unwrap()
                .as_ref(),
            [8, 32, 255, 255, 240, 128, 0, 255]
        );
        assert!(optical_flow_input_pixels(&rgba, ash::vk::Format::B8G8R8A8_SRGB).is_err());
    }

    #[test]
    #[cfg(feature = "nvidia-optical-flow")]
    fn adapter_enumeration_reports_optical_flow_support() {
        let _guard = VULKAN_TEST_LOCK.lock().unwrap();
        match enumerate_adapters() {
            Ok(adapters) => {
                assert!(!adapters.is_empty());
                assert!(adapters.iter().all(|adapter| !adapter.name.is_empty()));
                for adapter in adapters {
                    eprintln!(
                        "Vulkan adapter {:?}: optical-flow extension={}, queue={:?}",
                        adapter.name,
                        adapter.nvidia_optical_flow,
                        adapter.optical_flow_queue_family
                    );
                }
            }
            Err(error) => {
                eprintln!("Vulkan loader unavailable; skipping adapter probe: {error}");
            }
        }
    }

    #[test]
    #[cfg(feature = "nvidia-optical-flow")]
    fn creates_optical_flow_device_when_supported() {
        let _guard = VULKAN_TEST_LOCK.lock().unwrap();
        match OpticalFlowDevice::new() {
            Ok(device) => eprintln!(
                "Vulkan optical-flow device {:?}, queue family {}",
                device.name, device.queue_family_index
            ),
            Err(error) => eprintln!("Vulkan optical flow unavailable; skipping: {error}"),
        }
    }

    #[test]
    fn creates_sdl_surface_and_swapchain_when_a_display_is_available() {
        let _guard = VULKAN_TEST_LOCK.lock().unwrap();
        let sdl = match sdl3::init() {
            Ok(sdl) => sdl,
            Err(error) => {
                eprintln!("SDL unavailable; skipping surface test: {error}");
                return;
            }
        };
        let video = match sdl.video() {
            Ok(video) => video,
            Err(error) => {
                eprintln!("SDL video unavailable; skipping surface test: {error}");
                return;
            }
        };
        if let Err(error) = video.vulkan_load_library_default() {
            eprintln!("SDL Vulkan loader unavailable; skipping surface test: {error}");
            return;
        }
        let mut builder = video.window("Vulkan probe", 320, 240);
        builder.hidden().vulkan();
        let window = match builder.build() {
            Ok(window) => window,
            Err(error) => {
                eprintln!("SDL Vulkan window unavailable; skipping surface test: {error}");
                return;
            }
        };
        match SdlVulkanContext::new(&window) {
            Ok(mut context) => {
                assert!(!context.swapchain_images.is_empty());
                assert_eq!(
                    context.swapchain_images.len(),
                    context.swapchain_image_views.len()
                );
                let synthetic_image = crate::renderer::RgbaImage::new(
                    2,
                    2,
                    vec![
                        255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
                    ],
                )
                .unwrap();
                let texture_id = crate::renderer::RendererBackend::upload_texture(
                    &mut context,
                    &synthetic_image,
                )
                .expect("upload synthetic scene texture to Vulkan");
                assert_eq!(context.scene_texture_dimensions(texture_id), Some((2, 2)));
                let replacement_pixels = crate::renderer::RgbaImage::new(
                    2,
                    2,
                    vec![
                        32, 64, 96, 255, 32, 64, 96, 255, 32, 64, 96, 255, 32, 64, 96, 255,
                    ],
                )
                .unwrap();
                crate::renderer::RendererBackend::update_texture(
                    &mut context,
                    texture_id,
                    &replacement_pixels,
                )
                .expect("update synthetic texture in place");
                crate::renderer::RendererBackend::update_texture(
                    &mut context,
                    texture_id,
                    &synthetic_image,
                )
                .expect("queue consecutive in-place texture update");
                let updated_image =
                    crate::renderer::RgbaImage::new(1, 1, vec![10, 20, 30, 255]).unwrap();
                crate::renderer::RendererBackend::update_texture(
                    &mut context,
                    texture_id,
                    &updated_image,
                )
                .expect("resize synthetic Vulkan texture through renderer contract");
                assert_eq!(context.scene_texture_dimensions(texture_id), Some((1, 1)));
                crate::renderer::RendererBackend::update_texture(
                    &mut context,
                    texture_id,
                    &synthetic_image,
                )
                .expect("update synthetic scene texture through renderer contract");
                assert_eq!(context.scene_texture_dimensions(texture_id), Some((2, 2)));
                let pending_texture =
                    crate::renderer::RendererBackend::upload_texture(&mut context, &updated_image)
                        .expect("upload texture for immediate destruction");
                crate::renderer::RendererBackend::destroy_texture(&mut context, pending_texture)
                    .expect("destroy texture while its upload may still be pending");
                let mut draw_list = crate::renderer::DrawList::new(4, 2).unwrap();
                draw_list
                    .push(crate::renderer::DrawCommand::Clear(crate::renderer::Color(
                        [0, 0, 255, 255],
                    )))
                    .unwrap();
                draw_list
                    .push(crate::renderer::DrawCommand::Copy {
                        texture: texture_id,
                        source: None,
                        destination: crate::renderer::Rect {
                            x: 0.0,
                            y: 0.0,
                            width: 2.0,
                            height: 2.0,
                        },
                        alpha: 255,
                        filter: crate::renderer::TextureFilter::Nearest,
                        blend: crate::renderer::BlendMode::Opaque,
                    })
                    .unwrap();
                draw_list
                    .push(crate::renderer::DrawCommand::FillRect {
                        rect: crate::renderer::Rect {
                            x: 2.0,
                            y: 0.0,
                            width: 2.0,
                            height: 2.0,
                        },
                        color: crate::renderer::Color([0, 255, 0, 255]),
                        blend: crate::renderer::BlendMode::Opaque,
                    })
                    .unwrap();
                draw_list
                    .push(crate::renderer::DrawCommand::FillRect {
                        rect: crate::renderer::Rect {
                            x: 2.0,
                            y: 1.0,
                            width: 2.0,
                            height: 1.0,
                        },
                        color: crate::renderer::Color([255, 0, 0, 128]),
                        blend: crate::renderer::BlendMode::Alpha,
                    })
                    .unwrap();
                let composed_id = context
                    .compose_scene_offscreen(&draw_list)
                    .expect("compose synthetic Vulkan scene");
                let composed = context
                    .read_scene_texture_for_test(composed_id)
                    .expect("read back synthetic Vulkan scene for assertions");
                assert_eq!(&composed.rgba[0..4], &[255, 0, 0, 255]);
                assert_eq!(&composed.rgba[4..8], &[0, 255, 0, 255]);
                assert_eq!(&composed.rgba[8..12], &[0, 255, 0, 255]);
                assert_eq!(&composed.rgba[12..16], &[0, 255, 0, 255]);
                assert_eq!(&composed.rgba[16..20], &[0, 0, 255, 255]);
                assert_eq!(&composed.rgba[20..24], &[255, 255, 255, 255]);
                assert_eq!(&composed.rgba[24..28], &[128, 127, 0, 255]);
                assert_eq!(&composed.rgba[28..32], &[128, 127, 0, 255]);
                assert_eq!(
                    context
                        .compose_scene_offscreen(&draw_list)
                        .expect("reuse Vulkan scene target"),
                    composed_id
                );
                let mut feedback_draw_list = crate::renderer::DrawList::new(4, 2).unwrap();
                feedback_draw_list
                    .push(crate::renderer::DrawCommand::Clear(crate::renderer::Color(
                        [0, 0, 0, 255],
                    )))
                    .unwrap();
                feedback_draw_list
                    .push(crate::renderer::DrawCommand::Copy {
                        texture: composed_id,
                        source: None,
                        destination: crate::renderer::Rect {
                            x: 0.0,
                            y: 0.0,
                            width: 4.0,
                            height: 2.0,
                        },
                        alpha: 255,
                        filter: crate::renderer::TextureFilter::Nearest,
                        blend: crate::renderer::BlendMode::Opaque,
                    })
                    .unwrap();
                let feedback_output = context
                    .compose_scene_offscreen(&feedback_draw_list)
                    .expect("compose cached scene as an input without feedback");
                assert_ne!(feedback_output, composed_id);
                assert_eq!(
                    context
                        .read_scene_texture_for_test(feedback_output)
                        .expect("read feedback-safe scene output")
                        .rgba,
                    composed.rgba
                );
                crate::renderer::RendererBackend::submit(&mut context, &draw_list)
                    .expect("submit synthetic scene through Vulkan renderer contract");
                context
                    .destroy_scene_texture(feedback_output)
                    .expect("destroy feedback-safe output texture");
                context
                    .destroy_scene_texture(composed_id)
                    .expect("destroy composed synthetic scene texture");
                crate::renderer::RendererBackend::destroy_texture(&mut context, texture_id)
                    .expect("destroy synthetic scene texture through renderer contract");
                let mut fill_only = crate::renderer::DrawList::new(2, 2).unwrap();
                fill_only
                    .push(crate::renderer::DrawCommand::Clear(crate::renderer::Color(
                        [10, 20, 30, 255],
                    )))
                    .unwrap();
                fill_only
                    .push(crate::renderer::DrawCommand::FillRect {
                        rect: crate::renderer::Rect {
                            x: 0.0,
                            y: 0.0,
                            width: 1.0,
                            height: 2.0,
                        },
                        color: crate::renderer::Color([200, 100, 50, 255]),
                        blend: crate::renderer::BlendMode::Opaque,
                    })
                    .unwrap();
                let fill_output = context
                    .compose_scene_offscreen(&fill_only)
                    .expect("compose fill-only scene with persistent fallback");
                assert_eq!(
                    context
                        .read_scene_texture_for_test(fill_output)
                        .expect("read fill-only scene")
                        .rgba,
                    [
                        200, 100, 50, 255, 10, 20, 30, 255, 200, 100, 50, 255, 10, 20, 30, 255
                    ]
                );
                assert_eq!(
                    context
                        .compose_scene_offscreen(&fill_only)
                        .expect("reuse fill-only Vulkan resources"),
                    fill_output
                );
                context
                    .destroy_scene_texture(fill_output)
                    .expect("destroy fill-only scene output");
                let renderer = context
                    .create_sdl_renderer(&window)
                    .expect("create SDL renderer on the shared Vulkan device");
                let mut canvas =
                    unsafe { sdl3::render::Canvas::from_window_and_renderer(window, renderer) };
                let texture_creator = canvas.texture_creator();
                let scene = texture_creator
                    .create_texture_target(sdl3::pixels::PixelFormat::RGBA32, 320, 240)
                    .expect("create SDL Vulkan scene target");
                assert_ne!(
                    context
                        .sdl_texture_image(&scene)
                        .expect("retrieve SDL Vulkan scene image"),
                    ash::vk::Image::null()
                );
                canvas.set_draw_color(sdl3::pixels::Color::RGB(0, 0, 0));
                canvas.clear();
                canvas.present();
                #[cfg(feature = "nvidia-optical-flow")]
                {
                    let flow_formats = context
                        .optical_flow_input_formats()
                        .expect("query optical-flow input formats");
                    assert!(!flow_formats.is_empty());
                    let flow_session = context
                        .create_optical_flow_session(800, 600)
                        .expect("create NVIDIA optical-flow session");
                    assert_eq!(flow_session.width, 800);
                    assert_eq!(flow_session.height, 600);
                    assert!(flow_formats.contains(&flow_session.image_format));
                    let first_interpolation_started = Instant::now();
                    let first = context
                        .execute_optical_flow_probe()
                        .expect("execute NVIDIA optical-flow probe");
                    eprintln!(
                        "first synthetic Optical Flow at 800x600 took {:.2} ms including resource setup and readback",
                        first_interpolation_started.elapsed().as_secs_f64() * 1000.0
                    );
                    let repeated_interpolation_started = Instant::now();
                    let repeated = context
                        .execute_optical_flow_probe()
                        .expect("reuse NVIDIA optical-flow resources");
                    eprintln!(
                        "reused synthetic Optical Flow at 800x600 took {:.2} ms",
                        repeated_interpolation_started.elapsed().as_secs_f64() * 1000.0
                    );
                    assert_eq!(first, repeated);
                    eprintln!(
                        "synthetic Optical Flow mean vector at 800x600: ({:.2}, {:.2})",
                        first.0, first.1
                    );
                    eprintln!("synthetic midpoint mask pixel luminance: {}", first.2);
                    assert!(
                        (10.0..=14.0).contains(&first.0),
                        "expected +12px horizontal motion from translated probe, got {:.2}",
                        first.0
                    );
                    assert!(
                        first.1.abs() < 1.0,
                        "expected negligible vertical motion, got ({:.2}, {:.2})",
                        first.0,
                        first.1
                    );
                    assert!(
                        (220..=255).contains(&first.2),
                        "expected motion-compensated mask luminance near 240, got {}",
                        first.2
                    );
                    let previous = super::make_translation_probe_frame(800, 600, 0);
                    let next = super::make_translation_probe_frame(800, 600, 12);
                    let (reverse_image, reverse_flow) = context
                        .execute_optical_flow_frames(&next, &previous)
                        .expect("execute reversed NVIDIA optical-flow probe");
                    assert!(
                        (-14.0..=-10.0).contains(&reverse_flow.0),
                        "expected -12px horizontal motion for the reversed pair, got {:.2}",
                        reverse_flow.0
                    );
                    assert!(
                        reverse_flow.1.abs() < 1.0,
                        "expected negligible vertical motion for the reversed pair, got ({:.2}, {:.2})",
                        reverse_flow.0,
                        reverse_flow.1
                    );
                    let reverse_midpoint_luminance =
                        reverse_image.rgba[((300 * 800 + 400) * 4) as usize];
                    assert!(
                        (220..=255).contains(&reverse_midpoint_luminance),
                        "expected a stable midpoint mask for the reversed pair, got {reverse_midpoint_luminance}"
                    );
                }
                eprintln!(
                    "SDL Vulkan renderer created on {:?}, original swapchain {:?}, graphics queue {}",
                    context.name, context.swapchain, context.graphics_queue_family
                );
            }
            Err(error) => eprintln!("SDL Vulkan surface unavailable; skipping: {error}"),
        }
    }
}
