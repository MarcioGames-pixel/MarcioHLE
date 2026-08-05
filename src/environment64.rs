use crate::a64_runtime::{dispatch, materialize_import};
use crate::bundle::Bundle;
use crate::cpu::A64Cpu;
use crate::fs::Fs;
use crate::mach_o64::MachO64;
use crate::mem64::Mem64;
use crate::options::Options;
use std::collections::HashMap;
use touchHLE_dynarmic_wrapper::touchHLE_DynarmicA64Context;

const STACK_BASE: u64 = 0x7fff_ffff_0000;
const STACK_SIZE: u64 = 0x0010_0000;
const SVC_THREAD_EXIT: u32 = 1;
const SVC_RETURN_TO_HOST: u32 = 2;
const SVC_HOST_BASE: u32 = 0x100;
const HOST_STUB_SIZE: u64 = 8;
const MAX_HOST_DISPATCHES: u64 = 1_000_000;
const A64_HALT_USER_DEFINED1: u32 = 0x0100_0000;
const A64_HALT_USER_DEFINED2: u32 = 0x0200_0000;
const A64_HALT_USER_DEFINED3: u32 = 0x0400_0000;

fn put_string(mem: &mut Mem64, cursor: &mut u64, value: &str) -> Result<u64, String> {
    let bytes = value.as_bytes();
    *cursor = cursor.checked_sub(bytes.len() as u64 + 1).ok_or("ARM64 stack overflow")?;
    mem.write_bytes(*cursor, bytes).map_err(str::to_owned)?;
    mem.write_u8(*cursor + bytes.len() as u64, 0).map_err(str::to_owned)?;
    Ok(*cursor)
}

fn prepare_stack(
    mem: &mut Mem64,
    argv: &[String],
    envp: &[String],
    apple: &[String],
) -> Result<(u64, u64, u64, u64), String> {
    mem.map_zeroed(STACK_BASE - STACK_SIZE, STACK_SIZE).map_err(str::to_owned)?;
    let mut string_cursor = STACK_BASE & !15;
    let mut argv_strings = Vec::with_capacity(argv.len());
    let mut envp_strings = Vec::with_capacity(envp.len());
    let mut apple_strings = Vec::with_capacity(apple.len());
    for value in argv.iter().rev() {
        argv_strings.push(put_string(mem, &mut string_cursor, value)?);
    }
    for value in envp.iter().rev() {
        envp_strings.push(put_string(mem, &mut string_cursor, value)?);
    }
    for value in apple.iter().rev() {
        apple_strings.push(put_string(mem, &mut string_cursor, value)?);
    }
    let pointer_count = argv.len() + envp.len() + apple.len() + 4;
    let pointer_bytes = (pointer_count as u64)
        .checked_mul(8)
        .ok_or("ARM64 startup stack is too large")?;
    let sp = (string_cursor & !15)
        .checked_sub(pointer_bytes)
        .ok_or("ARM64 stack overflow")?
        & !15;
    let argc = argv.len() as u64;
    let argv_ptr = sp + 8;
    let envp_ptr = argv_ptr + ((argv.len() + 1) as u64 * 8);
    let apple_ptr = envp_ptr + ((envp.len() + 1) as u64 * 8);
    let mut cursor = sp;
    mem.write_u64(cursor, argc).map_err(str::to_owned)?;
    cursor += 8;
    for value in &argv_strings {
        mem.write_u64(cursor, *value).map_err(str::to_owned)?;
        cursor += 8;
    }
    mem.write_u64(cursor, 0).map_err(str::to_owned)?;
    cursor += 8;
    for value in &envp_strings {
        mem.write_u64(cursor, *value).map_err(str::to_owned)?;
        cursor += 8;
    }
    mem.write_u64(cursor, 0).map_err(str::to_owned)?;
    cursor += 8;
    for value in &apple_strings {
        mem.write_u64(cursor, *value).map_err(str::to_owned)?;
        cursor += 8;
    }
    mem.write_u64(cursor, 0).map_err(str::to_owned)?;
    Ok((sp, argv_ptr, envp_ptr, apple_ptr))
}

fn write_svc_stub(mem: &mut Mem64, svc: u32) -> Result<u64, String> {
    let stub = mem.alloc_zeroed(HOST_STUB_SIZE).map_err(str::to_owned)?;
    let instruction = 0xd4000001u32 | ((u64::from(svc) << 5) as u32);
    mem.write_u32(stub, instruction).map_err(str::to_owned)?;
    mem.write_u32(stub + 4, 0xd65f03c0).map_err(str::to_owned)?;
    Ok(stub)
}

fn lookup_host_symbol(symbol: &str) -> Option<&'static str> {
    crate::dyld::search_host_dylibs(|dylib| dylib.function_exports, symbol)
        .map(|(name, _)| *name)
}

pub fn run(bundle: Bundle, fs: Fs, options: Options, app_args: Vec<String>) -> Result<(), String> {
    echo!(
        "ARM64 launch configuration: device={:?}, orientation={:?}, fullscreen={}, screen={:?}, scale={:.2}, iOS={:?}",
        options.device_family,
        options.initial_orientation,
        options.fullscreen,
        options.host_screen_size,
        options.scale_hack,
        options.ios_version.unwrap_or(crate::options::LATEST_IOS_VERSION),
    );
    let executable_path = bundle.executable_path();
    let executable = MachO64::load_from_file(&executable_path, &fs, 0)?;
    let entry = executable.entry_point_pc.ok_or("ARM64 Mach-O has no entry point")?;
    let image_end = executable.last_segment_end;
    echo!("ARM64 image loaded: entry {:#x}, image range ends at {:#x}", entry, image_end);
    let mut memory = executable.memory;
    let argv = std::iter::once(executable_path.as_str().to_owned())
        .chain(app_args)
        .collect::<Vec<_>>();
    let apple = vec![format!("executable_path={}", executable_path.as_str())];
    let (sp, argv_ptr, envp_ptr, apple_ptr) = prepare_stack(&mut memory, &argv, &[], &apple)?;

    let return_stub = write_svc_stub(&mut memory, SVC_RETURN_TO_HOST)?;
    let mut host_stubs = HashMap::new();
    let mut stub_by_symbol = HashMap::new();
    let mut unresolved = Vec::new();
    let mut materialized_imports = 0usize;
    for (binding_index, binding) in executable.bindings.iter().enumerate() {
        if let Some(value) = materialize_import(&mut memory, &binding.symbol)? {
            if binding_index < 32 {
                echo!("ARM64 materialized import #{}: {} -> {:#x}", binding_index, binding.symbol, value);
            }
            memory.write_u64(binding.address, value.checked_add_signed(binding.addend).ok_or("ARM64 import address overflows")?).map_err(str::to_owned)?;
            materialized_imports += 1;
            continue;
        }
        let symbol = lookup_host_symbol(&binding.symbol)
            .or_else(|| lookup_host_symbol(binding.symbol.strip_prefix('_').unwrap_or(&binding.symbol)))
            .unwrap_or("<unimplemented>");
        if symbol == "<unimplemented>" && !crate::a64_runtime::can_dispatch(&binding.symbol) {
            unresolved.push(binding.symbol.clone());
        }
        let (svc, stub) = if let Some(&(svc, stub)) = stub_by_symbol.get(symbol) {
            (svc, stub)
        } else {
            let svc = SVC_HOST_BASE + host_stubs.len() as u32;
            let stub = write_svc_stub(&mut memory, svc)?;
            stub_by_symbol.insert(symbol, (svc, stub));
            host_stubs.insert(svc as i32, (binding.symbol.clone(), symbol));
            (svc, stub)
        };
        let target = stub.checked_add(binding.addend as u64).ok_or("ARM64 import target overflows")?;
        memory.write_u64(binding.address, target).map_err(str::to_owned)?;
    }

    echo!(
        "ARM64 runtime: entry point {:#x}, image_end {:#x}, {} host stubs, {} materialized imports, {} unresolved, stack {:#x}, argv {:#x}, envp {:#x}, apple {:#x}",
        entry,
        image_end,
        host_stubs.len(),
        unresolved.len(),
        sp,
        argv_ptr,
        envp_ptr,
        apple_ptr,
    );
    for symbol in unresolved.iter().take(32) {
        echo!("  unresolved import: {}", symbol);
    }
    if unresolved.len() > 32 {
        echo!("  ... and {} more unresolved imports", unresolved.len() - 32);
    }
    for (i, binding) in executable.bindings.iter().take(16).enumerate() {
        echo!("  {}: {} @ {:x} + {}", i, binding.symbol, binding.address, binding.addend);
    }
    if executable.bindings.len() > 16 {
        echo!("  ... and {} more", executable.bindings.len() - 16);
    }
    let mut context = touchHLE_DynarmicA64Context::default();
    context.sp = sp;
    context.pc = entry;
    context.regs[0] = argv.len() as u64;
    context.regs[1] = argv_ptr;
    context.regs[2] = envp_ptr;
    context.regs[3] = apple_ptr;
    context.regs[30] = return_stub;
    let mut cpu = A64Cpu::new();
    cpu.load_context(&context);
    let mut ticks = Some(100_000_u64);
    let mut host_dispatches = 0_u64;
    let mut last_pc = context.pc;
    let mut repeated_pc = 0_u64;
    loop {
        let result = cpu.run_or_step(&mut memory, ticks.as_mut());
        cpu.save_context(&mut context);
        match result {
            -1 => {
                ticks = Some(100_000);
                continue;
            }
            -2 => {
                return Err(format!(
                    "ARM64 guest memory fault at pc {:#x}, sp {:#x}, lr {:#x}, x0 {:#x}, x1 {:#x}, x2 {:#x}, x3 {:#x}",
                    context.pc,
                    context.sp,
                    context.regs[30],
                    context.regs[0],
                    context.regs[1],
                    context.regs[2],
                    context.regs[3],
                ));
            }
            -3 => {
                return Err(format!(
                    "ARM64 undefined instruction at pc {:#x}, sp {:#x}, lr {:#x}, x0 {:#x}, x1 {:#x}, x2 {:#x}, x3 {:#x}",
                    context.pc,
                    context.sp,
                    context.regs[30],
                    context.regs[0],
                    context.regs[1],
                    context.regs[2],
                    context.regs[3],
                ));
            }
            -4 => {
                return Err(format!(
                    "ARM64 breakpoint at pc {:#x}, sp {:#x}, lr {:#x}, x0 {:#x}, x1 {:#x}, x2 {:#x}, x3 {:#x}",
                    context.pc,
                    context.sp,
                    context.regs[30],
                    context.regs[0],
                    context.regs[1],
                    context.regs[2],
                    context.regs[3],
                ));
            }
            value if value == SVC_THREAD_EXIT as i32 || value == SVC_RETURN_TO_HOST as i32 => {
                echo!("ARM64 runtime returned from entry point");
                return Ok(());
            }
            value if value >= SVC_HOST_BASE as i32 => {
                host_dispatches += 1;
                if context.pc == last_pc {
                    repeated_pc += 1;
                } else {
                    last_pc = context.pc;
                    repeated_pc = 0;
                }
                if repeated_pc > 100_000 {
                    return Err(format!(
                        "ARM64 runtime stalled at pc {:#x}; last host binding was {}",
                        context.pc,
                        host_stubs.get(&value).map(|(name, _)| name.as_str()).unwrap_or("<unknown>"),
                    ));
                }
                let symbol = host_stubs.get(&value).map(|(name, _)| name.as_str()).unwrap_or("<unknown>");
                if host_dispatches <= 128 || host_dispatches.is_power_of_two() {
                    echo!(
                        "ARM64 host binding #{}: {} pc={:#x} sp={:#x} lr={:#x} x0={:#x} x1={:#x} x2={:#x} x3={:#x} x4={:#x} x5={:#x}",
                        host_dispatches,
                        symbol,
                        context.pc,
                        context.sp,
                        context.regs[30],
                        context.regs[0],
                        context.regs[1],
                        context.regs[2],
                        context.regs[3],
                        context.regs[4],
                        context.regs[5],
                    );
                }
                let handled = dispatch(&mut memory, &mut context, symbol)?;
                if !handled {
                    echo!("Warning: ARM64 host function {} is not implemented; returning zero", symbol);
                }
                if host_dispatches > MAX_HOST_DISPATCHES {
                    return Err(format!("ARM64 runtime made too many host calls; last binding was {}", symbol));
                }
                context.pc = context.regs[30];
                cpu.load_context(&context);
                cpu.clear_halt(A64_HALT_USER_DEFINED1);
                cpu.clear_halt(A64_HALT_USER_DEFINED2);
                cpu.clear_halt(A64_HALT_USER_DEFINED3);
                continue;
            }
            value if value >= 0 => return Err(format!("ARM64 runtime reached unimplemented SVC {} at {:#x}", value, context.pc)),
            value => return Err(format!("ARM64 runtime failed with code {} at {:#x}", value, context.pc)),
        }
    }
}
