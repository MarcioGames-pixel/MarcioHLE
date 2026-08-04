use crate::a64_abi::A64Abi;
use crate::mem64::Mem64;
use touchHLE_dynarmic_wrapper::touchHLE_DynarmicA64Context;

const MAX_CSTRING: u64 = 1024 * 1024;
const A64_OBJECT_SIZE: u64 = 96;
const A64_KIND_CLASS: u64 = 1;
const A64_KIND_DEVICE: u64 = 2;
const A64_KIND_QUEUE: u64 = 3;
const A64_KIND_COMMAND_BUFFER: u64 = 4;
const A64_KIND_RENDER_ENCODER: u64 = 5;
const A64_KIND_COMPUTE_ENCODER: u64 = 6;
const A64_KIND_BLIT_ENCODER: u64 = 7;
const A64_KIND_BUFFER: u64 = 8;
const A64_KIND_TEXTURE: u64 = 9;
const A64_KIND_TEXTURE_DESCRIPTOR: u64 = 10;
const A64_KIND_STRING: u64 = 11;
const A64_KIND_PIPELINE: u64 = 12;
const A64_KIND_GENERIC: u64 = 13;

fn name(symbol: &str) -> &str {
    symbol.trim_start_matches('_')
}

fn return_value(context: &mut touchHLE_DynarmicA64Context, value: u64) {
    A64Abi::set_return(context, value);
}

fn c_string(mem: &Mem64, address: u64) -> Option<Vec<u8>> {
    let length = mem.cstr_len(address, MAX_CSTRING).ok()?;
    mem.read_bytes(address, length).ok()
}

fn c_string_eq(mem: &Mem64, address: u64, value: &[u8]) -> bool {
    c_string(mem, address).as_deref() == Some(value)
}

fn objc_object(mem: &mut Mem64, kind: u64) -> Result<u64, String> {
    let address = mem.alloc_zeroed(A64_OBJECT_SIZE).map_err(str::to_owned)?;
    mem.write_u64(address, kind).map_err(str::to_owned)?;
    Ok(address)
}

fn objc_kind(mem: &Mem64, address: u64) -> Option<u64> {
    (address != 0 && mem.allocation_size(address).is_some())
        .then(|| mem.read_u64(address).ok())
        .flatten()
}

fn objc_field(mem: &Mem64, object: u64, offset: u64) -> u64 {
    mem.read_u64(object.saturating_add(offset)).unwrap_or(0)
}

fn set_objc_field(mem: &mut Mem64, object: u64, offset: u64, value: u64) {
    let _ = mem.write_u64(object.saturating_add(offset), value);
}

fn objc_string(mem: &mut Mem64, value: &str) -> Result<u64, String> {
    let object = objc_object(mem, A64_KIND_STRING)?;
    let bytes = value.as_bytes();
    let pointer = mem.alloc_zeroed(bytes.len() as u64 + 1).map_err(str::to_owned)?;
    mem.write_bytes(pointer, bytes).map_err(str::to_owned)?;
    mem.write_u8(pointer + bytes.len() as u64, 0).map_err(str::to_owned)?;
    set_objc_field(mem, object, 56, pointer);
    set_objc_field(mem, object, 64, bytes.len() as u64);
    Ok(object)
}

fn objc_class_kind(mem: &Mem64, class_name: u64) -> u64 {
    match c_string(mem, class_name).as_deref() {
        Some(b"MTLDevice") => A64_KIND_DEVICE,
        Some(b"MTLCommandQueue") => A64_KIND_QUEUE,
        Some(b"MTLCommandBuffer") => A64_KIND_COMMAND_BUFFER,
        Some(b"MTLRenderCommandEncoder") => A64_KIND_RENDER_ENCODER,
        Some(b"MTLComputeCommandEncoder") => A64_KIND_COMPUTE_ENCODER,
        Some(b"MTLBlitCommandEncoder") => A64_KIND_BLIT_ENCODER,
        Some(b"MTLBuffer") => A64_KIND_BUFFER,
        Some(b"MTLTexture") => A64_KIND_TEXTURE,
        Some(b"MTLTextureDescriptor") => A64_KIND_TEXTURE_DESCRIPTOR,
        Some(b"MTLRenderPipelineState") => A64_KIND_PIPELINE,
        _ => A64_KIND_GENERIC,
    }
}

fn objc_send(mem: &mut Mem64, context: &mut touchHLE_DynarmicA64Context) -> Result<(), String> {
    let receiver = context.regs[0];
    let selector = c_string(mem, context.regs[1])
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_default();
    let kind = objc_kind(mem, receiver).unwrap_or(A64_KIND_GENERIC);
    let class_name = objc_field(mem, receiver, 56);

    let result = match selector.as_str() {
        "init" | "self" | "retain" | "autorelease" | "copy" | "mutableCopy" => receiver,
        "release" => 0,
        "class" => receiver,
        "respondsToSelector:" | "isKindOfClass:" | "hasUnifiedMemory" => 1,
        "supportsFamily:" | "supportsFeatureSet:" => 1,
        "supportsTextureSampleCount:" => u64::from(matches!(context.regs[2], 1 | 2 | 4)),
        "name" => objc_string(mem, "RadekHLE Metal device")?,
        "UTF8String" => objc_field(mem, receiver, 56),
        "length" if kind == A64_KIND_STRING || kind == A64_KIND_BUFFER => objc_field(mem, receiver, 64),
        "newCommandQueue" | "newCommandQueueWithMaxCommandBufferCount:" => objc_object(mem, A64_KIND_QUEUE)?,
        "commandBuffer" | "commandBufferWithUnretainedReferences" => objc_object(mem, A64_KIND_COMMAND_BUFFER)?,
        "renderCommandEncoderWithDescriptor:" => objc_object(mem, A64_KIND_RENDER_ENCODER)?,
        "computeCommandEncoder" => objc_object(mem, A64_KIND_COMPUTE_ENCODER)?,
        "blitCommandEncoder" => objc_object(mem, A64_KIND_BLIT_ENCODER)?,
        "newRenderPipelineStateWithDescriptor:error:" => objc_object(mem, A64_KIND_PIPELINE)?,
        "newDepthStencilStateWithDescriptor:" | "newSamplerStateWithDescriptor:" => objc_object(mem, A64_KIND_GENERIC)?,
        "newBufferWithLength:options:" => {
            let object = objc_object(mem, A64_KIND_BUFFER)?;
            let length = context.regs[2];
            let contents = mem.alloc_zeroed(length).map_err(str::to_owned)?;
            set_objc_field(mem, object, 56, contents);
            set_objc_field(mem, object, 64, length);
            set_objc_field(mem, object, 72, context.regs[3]);
            set_objc_field(mem, object, 80, receiver);
            object
        }
        "newBufferWithBytes:length:options:" => {
            let object = objc_object(mem, A64_KIND_BUFFER)?;
            let source = context.regs[2];
            let length = context.regs[3];
            let contents = mem.alloc_zeroed(length).map_err(str::to_owned)?;
            if source != 0 && length != 0 {
                let bytes = mem.read_bytes(source, length).map_err(str::to_owned)?;
                mem.write_bytes(contents, &bytes).map_err(str::to_owned)?;
            }
            set_objc_field(mem, object, 56, contents);
            set_objc_field(mem, object, 64, length);
            set_objc_field(mem, object, 72, context.regs[4]);
            set_objc_field(mem, object, 80, receiver);
            object
        }
        "newTextureWithDescriptor:" => {
            let object = objc_object(mem, A64_KIND_TEXTURE)?;
            for offset in [8, 16, 24, 32, 40, 48] {
                set_objc_field(mem, object, offset, objc_field(mem, context.regs[2], offset));
            }
            set_objc_field(mem, object, 80, receiver);
            object
        }
        "texture2DDescriptorWithPixelFormat:width:height:mipmapped:" => {
            let object = objc_object(mem, A64_KIND_TEXTURE_DESCRIPTOR)?;
            set_objc_field(mem, object, 8, context.regs[2]);
            set_objc_field(mem, object, 16, context.regs[3]);
            set_objc_field(mem, object, 24, context.regs[4]);
            set_objc_field(mem, object, 32, 1);
            set_objc_field(mem, object, 40, if context.regs[5] != 0 { 0 } else { 1 });
            set_objc_field(mem, object, 48, 1);
            object
        }
        "pixelFormat" => objc_field(mem, receiver, 8),
        "width" => objc_field(mem, receiver, 16),
        "height" => objc_field(mem, receiver, 24),
        "depth" => objc_field(mem, receiver, 32),
        "mipmapLevelCount" => objc_field(mem, receiver, 40),
        "sampleCount" => objc_field(mem, receiver, 48),
        "contents" => objc_field(mem, receiver, 56),
        "storageMode" => objc_field(mem, receiver, 72),
        "device" => objc_field(mem, receiver, 80),
        selector if selector.starts_with("set") && selector.ends_with(':') => {
            let value = context.regs[2];
            let offset = match selector {
                "setPixelFormat:" => 8,
                "setWidth:" => 16,
                "setHeight:" => 24,
                "setDepth:" => 32,
                "setMipmapLevelCount:" => 40,
                "setSampleCount:" => 48,
                _ => 72,
            };
            set_objc_field(mem, receiver, offset, value);
            0
        }
        "alloc" | "new" if kind == A64_KIND_CLASS => objc_object(mem, objc_class_kind(mem, class_name))?,
        _ => 0,
    };
    return_value(context, result);
    Ok(())
}

fn objc_class(mem: &mut Mem64, name: u64) -> Result<u64, String> {
    let object = objc_object(mem, A64_KIND_CLASS)?;
    set_objc_field(mem, object, 56, name);
    Ok(object)
}

pub fn dispatch(
    mem: &mut Mem64,
    context: &mut touchHLE_DynarmicA64Context,
    symbol: &str,
) -> Result<bool, String> {
    let symbol = name(symbol);
    match symbol {
        "malloc" | "calloc" | "valloc" | "posix_memalign" => {
            let size = if symbol == "calloc" {
                A64Abi::arg(context, 0)
                    .checked_mul(A64Abi::arg(context, 1))
                    .ok_or("ARM64 calloc size overflows")?
            } else if symbol == "posix_memalign" {
                A64Abi::arg(context, 2)
            } else {
                A64Abi::arg(context, 0)
            };
            let address = mem.alloc_zeroed(size).map_err(str::to_owned)?;
            if symbol == "posix_memalign" {
                mem.write_u64(context.regs[0], address).map_err(str::to_owned)?;
                return_value(context, 0);
            } else {
                return_value(context, address);
            }
            Ok(true)
        }
        "free" | "malloc_zone_free" => {
            if context.regs[0] != 0 {
                mem.free(context.regs[0]);
            }
            return_value(context, 0);
            Ok(true)
        }
        "objc_release" | "objc_storeStrong" => {
            return_value(context, 0);
            Ok(true)
        }
        "realloc" | "malloc_zone_realloc" => {
            let old = context.regs[0];
            let size = context.regs[1];
            let address = mem.alloc_zeroed(size).map_err(str::to_owned)?;
            if old != 0 {
                if let Some(old_size) = mem.allocation_size(old) {
                    mem.copy_bytes(address, old, old_size.min(size)).map_err(str::to_owned)?;
                }
            }
            return_value(context, address);
            Ok(true)
        }
        "memcpy" | "memmove" | "__memcpy_chk" | "__memmove_chk" => {
            let size = if symbol.starts_with("__") { context.regs[2] } else { context.regs[2] };
            mem.copy_bytes(context.regs[0], context.regs[1], size).map_err(str::to_owned)?;
            return_value(context, context.regs[0]);
            Ok(true)
        }
        "memset" | "bzero" | "__memset_chk" => {
            let size = if symbol == "bzero" { context.regs[1] } else { context.regs[2] };
            let value = if symbol == "bzero" { 0 } else { context.regs[1] as u8 };
            mem.fill_bytes(context.regs[0], value, size).map_err(str::to_owned)?;
            return_value(context, context.regs[0]);
            Ok(true)
        }
        "strlen" => {
            return_value(context, mem.cstr_len(context.regs[0], 1024 * 1024).map_err(str::to_owned)?);
            Ok(true)
        }
        "strcmp" | "strncmp" => {
            let left = c_string(mem, context.regs[0]).unwrap_or_default();
            let right = c_string(mem, context.regs[1]).unwrap_or_default();
            let limit = if symbol == "strncmp" { context.regs[2] as usize } else { usize::MAX };
            let result = left.iter().take(limit).zip(right.iter().take(limit)).find_map(|(a, b)| (a != b).then_some((*a as i32) - (*b as i32))).unwrap_or_else(|| {
                let left_len = left.len().min(limit);
                let right_len = right.len().min(limit);
                (left_len as i32) - (right_len as i32)
            });
            return_value(context, result as i64 as u64);
            Ok(true)
        }
        "memcmp" => {
            let size = context.regs[2];
            let left = mem.read_bytes(context.regs[0], size).map_err(str::to_owned)?;
            let right = mem.read_bytes(context.regs[1], size).map_err(str::to_owned)?;
            let result = left.iter().zip(right.iter()).find_map(|(a, b)| (a != b).then_some((*a as i32) - (*b as i32))).unwrap_or(0);
            return_value(context, result as i64 as u64);
            Ok(true)
        }
        "objc_retain" | "objc_retainAutoreleasedReturnValue" | "objc_retainAutoreleaseReturnValue" | "objc_autorelease" | "objc_autoreleaseReturnValue" | "objc_unsafeClaimAutoreleasedReturnValue" | "objc_retainAutorelease" | "objc_retainBlock" => {
            let value = context.regs[0];
            return_value(context, value);
            Ok(true)
        }
        "objc_msgSend" | "objc_msgSendSuper2" | "objc_msgSend_stret" | "objc_msgSendSuper2_stret" => {
            objc_send(mem, context)?;
            Ok(true)
        }
        "objc_getClass" | "objc_getRequiredClass" | "objc_lookUpClass" => {
            let class = objc_class(mem, context.regs[0])?;
            return_value(context, class);
            Ok(true)
        }
        "sel_registerName" | "sel_getUid" => {
            return_value(context, context.regs[0]);
            Ok(true)
        }
        "objc_autoreleasePoolPush" => {
            let address = mem.alloc_zeroed(8).map_err(str::to_owned)?;
            return_value(context, address);
            Ok(true)
        }
        "objc_autoreleasePoolPop" | "objc_exception_throw" | "objc_begin_catch" | "objc_end_catch" => {
            return_value(context, 0);
            Ok(true)
        }
        "__cxa_atexit" | "atexit" | "pthread_mutex_lock" | "pthread_mutex_unlock" | "pthread_mutex_init" | "pthread_mutex_destroy" => {
            return_value(context, 0);
            Ok(true)
        }
        "NSLog" | "NSLogv" | "os_log" | "os_logv" => {
            return_value(context, 0);
            Ok(true)
        }
        "__CFConstantStringClassReference" => {
            return_value(context, 0);
            Ok(true)
        }
        _ if c_string_eq(mem, context.regs[0], b"NSConcreteGlobalBlock") || c_string_eq(mem, context.regs[0], b"NSConcreteStackBlock") => {
            return_value(context, 0);
            Ok(true)
        }
        "MTLCreateSystemDefaultDevice" => {
            return_value(context, mem.alloc_zeroed(8).map_err(str::to_owned)?);
            Ok(true)
        }
        "vkEnumerateInstanceVersion" => {
            if context.regs[0] != 0 {
                mem.write_u32(context.regs[0], (1 << 22) | (3 << 12)).map_err(str::to_owned)?;
            }
            return_value(context, 0);
            Ok(true)
        }
        "vkCreateInstance" => {
            if context.regs[2] == 0 {
                return_value(context, u64::MAX);
            } else {
                let handle = mem.alloc_zeroed(8).map_err(str::to_owned)?;
                mem.write_u64(context.regs[2], handle).map_err(str::to_owned)?;
                return_value(context, 0);
            }
            Ok(true)
        }
        "vkDestroyInstance" | "vkDestroyDevice" => {
            return_value(context, 0);
            Ok(true)
        }
        "vkEnumeratePhysicalDevices" => {
            if context.regs[1] == 0 {
                return_value(context, u64::MAX);
            } else {
                mem.write_u32(context.regs[1], 1).map_err(str::to_owned)?;
                if context.regs[2] != 0 {
                    let handle = mem.alloc_zeroed(8).map_err(str::to_owned)?;
                    mem.write_u64(context.regs[2], handle).map_err(str::to_owned)?;
                }
                return_value(context, 0);
            }
            Ok(true)
        }
        "vkGetPhysicalDeviceQueueFamilyProperties" => {
            if context.regs[1] != 0 {
                mem.write_u32(context.regs[1], 1).map_err(str::to_owned)?;
                if context.regs[2] != 0 {
                    let values = [1u32, 1, 64, 1, 1, 1];
                    for (index, value) in values.iter().enumerate() {
                        mem.write_u32(context.regs[2] + (index as u64 * 4), *value).map_err(str::to_owned)?;
                    }
                }
            }
            return_value(context, 0);
            Ok(true)
        }
        "vkCreateDevice" => {
            if context.regs[3] == 0 {
                return_value(context, u64::MAX);
            } else {
                let handle = mem.alloc_zeroed(8).map_err(str::to_owned)?;
                mem.write_u64(context.regs[3], handle).map_err(str::to_owned)?;
                return_value(context, 0);
            }
            Ok(true)
        }
        "vkGetDeviceQueue" => {
            if context.regs[3] != 0 {
                let handle = mem.alloc_zeroed(8).map_err(str::to_owned)?;
                mem.write_u64(context.regs[3], handle).map_err(str::to_owned)?;
            }
            return_value(context, 0);
            Ok(true)
        }
        "vkDeviceWaitIdle" => {
            return_value(context, 0);
            Ok(true)
        }
        _ if symbol.starts_with("gl") || symbol.starts_with("egl") || symbol.starts_with("EAGL") => {
            return_value(context, 0);
            Ok(true)
        }
        _ => Ok(false),
    }
}
