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

fn put_string(mem: &mut Mem64, cursor: &mut u64, value: &str) -> Result<u64, String> {
    let bytes = value.as_bytes();
    *cursor = cursor.checked_sub(bytes.len() as u64 + 1).ok_or("ARM64 stack overflow")?;
    mem.write_bytes(*cursor, bytes).map_err(str::to_owned)?;
    mem.write_u8(*cursor + bytes.len() as u64, 0).map_err(str::to_owned)?;
    Ok(*cursor)
}

fn push_u64(mem: &mut Mem64, cursor: &mut u64, value: u64) -> Result<(), String> {
    *cursor = cursor.checked_sub(8).ok_or("ARM64 stack overflow")?;
    mem.write_u64(*cursor, value).map_err(str::to_owned)
}

fn prepare_stack(mem: &mut Mem64, argv: &[String], envp: &[String], apple: &[String]) -> Result<(u64, u64, u64, u64), String> {
    mem.map_zeroed(STACK_BASE - STACK_SIZE, STACK_SIZE).map_err(str::to_owned)?;
    let mut cursor = STACK_BASE & !15;
    let mut argv_strings = Vec::with_capacity(argv.len());
    let mut envp_strings = Vec::with_capacity(envp.len());
    let mut apple_strings = Vec::with_capacity(apple.len());
    for value in argv.iter().rev() {
        argv_strings.push(put_string(mem, &mut cursor, value)?);
    }
    for value in envp.iter().rev() {
        envp_strings.push(put_string(mem, &mut cursor, value)?);
    }
    for value in apple.iter().rev() {
        apple_strings.push(put_string(mem, &mut cursor, value)?);
    }
    cursor &= !15;
    let argv_ptr = cursor - ((argv.len() + 1) as u64 * 8);
    let envp_ptr = argv_ptr - ((envp.len() + 1) as u64 * 8);
    let apple_ptr = envp_ptr - ((apple.len() + 1) as u64 * 8);
    for value in argv_strings.iter().rev() {
        push_u64(mem, &mut cursor, *value)?;
    }
    push_u64(mem, &mut cursor, 0)?;
    for value in envp_strings.iter().rev() {
        push_u64(mem, &mut cursor, *value)?;
    }
    push_u64(mem, &mut cursor, 0)?;
    for value in apple_strings.iter().rev() {
        push_u64(mem, &mut cursor, *value)?;
    }
    push_u64(mem, &mut cursor, 0)?;
    cursor &= !15;
    let sp = cursor;
    Ok((sp, argv_ptr, envp_ptr, apple_ptr))
}

pub fn run(bundle: Bundle, fs: Fs, options: Options, app_args: Vec<String>) -> Result<(), String> {
    let executable_path = bundle.executable_path();
    let executable = MachO64::load_from_file(&executable_path, &fs, 0)?;
    let entry = executable.entry_point_pc.ok_or("ARM64 Mach-O has no entry point")?;
    let mut memory = executable.memory;
    let argv = std::iter::once(bundle.executable_path().as_str().to_owned())
        .chain(app_args)
        .collect::<Vec<_>>();
    let apple = vec![format!("executable_path={}", executable_path.as_str())];
    let (sp, argv_ptr, envp_ptr, apple_ptr) = prepare_stack(&mut memory, &argv, &[], &apple)?;
    let mut context = touchHLE_DynarmicA64Context::default();
    context.sp = sp;
    context.pc = entry;
    context.regs[0] = argv.len() as u64;
    context.regs[1] = argv_ptr;
    context.regs[2] = envp_ptr;
    context.regs[3] = apple_ptr;
    let mut cpu = A64Cpu::new();
    cpu.swap_context(&mut context);
    let mut ticks = Some(100_000_u64);
    let mut first_step = true;
    let mut host_stubs = HashMap::new();
    for binding in &executable.bindings {
        let svc = SVC_HOST_BASE + host_stubs.len() as u32;
        let stub = memory.alloc_zeroed(8).map_err(str::to_owned)?;
        memory.write_u32(stub, 0xd4000001 | (u64::from(svc) << 5) as u32).map_err(str::to_owned)?;
        memory.write_u32(stub + 4, 0xd65f03c0).map_err(str::to_owned)?;
        memory.write_u64(binding.address, stub).map_err(str::to_owned)?;
        host_stubs.insert(svc as i32, binding.symbol.clone());
    }
    loop {
        let result = cpu.run_or_step(&mut memory, ticks.as_mut());
        cpu.swap_context(&mut context);
        match result {
            -1 => {
                if first_step {
                    return Err("ARM64 runtime stopped before executing the entry point".to_string());
                }
            }
            -2 => return Err(format!("ARM64 guest memory fault at {:#x}", context.pc)),
            -3 => return Err(format!("ARM64 undefined instruction at {:#x}", context.pc)),
            -4 => return Err(format!("ARM64 breakpoint at {:#x}", context.pc)),
            value if value == SVC_THREAD_EXIT as i32 || value == SVC_RETURN_TO_HOST as i32 => return Ok(()),
            value if value >= SVC_HOST_BASE as i32 => {
                if let Some(symbol) = host_stubs.get(&value) {
                    log!("ARM64 host binding fallback for {}", symbol);
                }
                context.pc = context.regs[30];
                continue;
            }
            value if value >= 0 => return Err(format!("ARM64 runtime reached unimplemented SVC {} at {:#x}", value, context.pc)),
            value => return Err(format!("ARM64 runtime failed with code {} at {:#x}", value, context.pc)),
        }
        first_step = false;
        ticks = Some(100_000);
        let _ = &options;
    }
}
