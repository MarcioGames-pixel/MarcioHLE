use std::collections::HashMap;
use std::io::{Cursor, Seek, SeekFrom};

use mach_object::{Bind, BindSymbolType, LazyBind, LoadCommand, MachCommand, OFile, Rebase, Symbol, SymbolIter, ThreadState, WeakBind};

use crate::mem64::{Guest64Addr, Mem64};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Architecture {
    Arm64,
}

#[derive(Debug, Clone)]
pub struct Binding64 {
    pub address: Guest64Addr,
    pub symbol: String,
}

#[derive(Debug)]
pub struct MachO64 {
    pub architecture: Architecture,
    pub name: String,
    pub dynamic_libraries: Vec<String>,
    pub exported_symbols: HashMap<String, Guest64Addr>,
    pub bindings: Vec<Binding64>,
    pub entry_point_pc: Option<Guest64Addr>,
    pub text_base: Guest64Addr,
    pub last_segment_end: Guest64Addr,
    pub memory: Mem64,
}

fn command_bytes<'a>(bytes: &'a [u8], offset: u32, size: u32) -> Result<&'a [u8], String> {
    let start = usize::try_from(offset).map_err(|_| "ARM64 dyld info offset is too large")?;
    let length = usize::try_from(size).map_err(|_| "ARM64 dyld info size is too large")?;
    let end = start.checked_add(length).ok_or("ARM64 dyld info range overflows")?;
    bytes.get(start..end).ok_or_else(|| "ARM64 dyld info extends past the Mach-O file".to_string())
}

impl MachO64 {
    pub fn load_from_file<P: AsRef<crate::fs::GuestPath>>(
        path: P,
        fs: &crate::fs::Fs,
        slide: u64,
    ) -> Result<Self, String> {
        let name = path
            .as_ref()
            .file_name()
            .ok_or("64-bit executable has no file name")?
            .to_string();
        let bytes = fs.read(path.as_ref()).map_err(|_| "Could not read 64-bit executable file")?;
        Self::load_from_bytes(&bytes, name, slide)
    }

    pub fn load_from_bytes(bytes: &[u8], name: impl Into<String>, slide: u64) -> Result<Self, String> {
        let name = name.into();
        let mut cursor = Cursor::new(bytes);
        let file = OFile::parse(&mut cursor).map_err(|e| format!("could not parse ARM64 Mach-O: {e}"))?;
        let (image_bytes, file) = match file {
            OFile::FatFile { files, .. } => {
                let (arch, file) = files
                    .into_iter()
                    .find(|(arch, _)| arch.cputype == mach_object::CPU_TYPE_ARM64)
                    .ok_or("fat binary has no ARM64 slice")?;
                let start = usize::try_from(arch.offset).map_err(|_| "ARM64 fat slice offset is too large")?;
                let length = usize::try_from(arch.size).map_err(|_| "ARM64 fat slice size is too large")?;
                let end = start.checked_add(length).ok_or("ARM64 fat slice range overflows")?;
                (bytes.get(start..end).ok_or("ARM64 fat slice extends past the file")?, file)
            }
            file => (bytes, file),
        };
        let (header, commands) = match file {
            OFile::MachFile { header, commands } => (header, commands),
            _ => return Err("ARM64 input is not an executable Mach-O".into()),
        };
        if header.cputype != mach_object::CPU_TYPE_ARM64 || !header.is_64bit() {
            return Err("Mach-O is not an ARM64 64-bit image".into());
        }
        if header.is_bigend() {
            return Err("ARM64 Mach-O is big-endian".into());
        }

        let mut memory = Mem64::new();
        let mut dynamic_libraries = Vec::new();
        let mut exported_symbols = HashMap::new();
        let mut bindings = Vec::new();
        let mut text_base = None;
        let mut last_segment_end = 0;
        let mut entry_point_pc = None;
        let mut entry_point_offset = None;
        let mut symtab = None;
        let mut sections = Vec::new();
        let mut segment_bases = Vec::new();

        for MachCommand(command, _) in commands {
            match command {
                LoadCommand::Segment64 {
                    segname,
                    vmaddr,
                    vmsize,
                    fileoff,
                    filesize,
                    sections: segment_sections,
                    ..
                } => {
                    let base = (vmaddr as u64).checked_add(slide).ok_or("segment address overflows")?;
                    segment_bases.push(base);
                    last_segment_end = last_segment_end
                        .max(base.checked_add(vmsize as u64).ok_or("segment end overflows")?);
                    if segname == "__PAGEZERO" {
                        continue;
                    }
                    if segname == "__TEXT" {
                        text_base = Some(base);
                    }
                    if vmsize == 0 {
                        continue;
                    }
                    memory.map_zeroed(base, vmsize as u64)?;
                    if filesize != 0 {
                        let start = usize::try_from(fileoff).map_err(|_| "segment file offset is too large")?;
                        let length = usize::try_from(filesize).map_err(|_| "segment file size is too large")?;
                        let end = start.checked_add(length).ok_or("segment file range overflows")?;
                        let source = image_bytes.get(start..end).ok_or_else(|| format!("segment {segname} extends past the Mach-O file"))?;
                        memory.write_bytes(base, source)?;
                    }
                    sections.extend(segment_sections);
                }
                LoadCommand::SymTab { symoff, nsyms, stroff, strsize } => {
                    symtab = Some((symoff, nsyms, stroff, strsize));
                }
                LoadCommand::LoadDyLib(lib) => dynamic_libraries.push(lib.name.to_string()),
                LoadCommand::EncryptionInfo64 { id, .. } if id != 0 => {
                    return Err("ARM64 executable is encrypted".into());
                }
                LoadCommand::EntryPoint { entryoff, .. } => entry_point_offset = Some(entryoff),
                LoadCommand::UnixThread { state: ThreadState::Arm64 { __pc, .. }, .. } => {
                    entry_point_pc = Some(__pc.checked_add(slide).ok_or("entry point overflows")?);
                }
                LoadCommand::DyldInfo {
                    rebase_off,
                    rebase_size,
                    bind_off,
                    bind_size,
                    weak_bind_off,
                    weak_bind_size,
                    lazy_bind_off,
                    lazy_bind_size,
                    ..
                } => {
                    for rebased in Rebase::parse(command_bytes(image_bytes, rebase_off, rebase_size)?, 8) {
                        if rebased.symbol_type != BindSymbolType::Pointer {
                            continue;
                        }
                        let segment = *segment_bases
                            .get(rebased.segment_index)
                            .ok_or("ARM64 rebase references an invalid segment")?;
                        let address = segment
                            .checked_add(rebased.symbol_offset as u64)
                            .ok_or("ARM64 rebase address overflows")?;
                        let value = memory.read_u64(address)?;
                        memory.write_u64(address, value.checked_add(slide).ok_or("ARM64 rebase value overflows")?)?;
                    }
                    for bound in Bind::parse(command_bytes(image_bytes, bind_off, bind_size)?, 8) {
                        if bound.symbol_type != BindSymbolType::Pointer {
                            continue;
                        }
                        let segment = *segment_bases
                            .get(bound.segment_index)
                            .ok_or("ARM64 bind references an invalid segment")?;
                        let address = segment
                            .checked_add(bound.symbol_offset as u64)
                            .ok_or("ARM64 bind address overflows")?;
                        bindings.push(Binding64 { address, symbol: bound.name });
                    }
                    for bound in WeakBind::parse(command_bytes(image_bytes, weak_bind_off, weak_bind_size)?, 8) {
                        if bound.symbol_type != BindSymbolType::Pointer {
                            continue;
                        }
                        let segment = *segment_bases
                            .get(bound.segment_index)
                            .ok_or("ARM64 weak bind references an invalid segment")?;
                        let address = segment
                            .checked_add(bound.symbol_offset as u64)
                            .ok_or("ARM64 weak bind address overflows")?;
                        bindings.push(Binding64 { address, symbol: bound.name });
                    }
                    for bound in LazyBind::parse(command_bytes(image_bytes, lazy_bind_off, lazy_bind_size)?, 8) {
                        let segment = *segment_bases
                            .get(bound.segment_index)
                            .ok_or("ARM64 lazy bind references an invalid segment")?;
                        let address = segment
                            .checked_add(bound.symbol_offset as u64)
                            .ok_or("ARM64 lazy bind address overflows")?;
                        bindings.push(Binding64 { address, symbol: bound.name });
                    }
                }
                _ => {}
            }
        }

        if let Some(entryoff) = entry_point_offset {
            entry_point_pc = Some(
                text_base
                    .ok_or("ARM64 LC_MAIN image has no __TEXT segment")?
                    .checked_add(entryoff)
                    .ok_or("entry point overflows")?,
            );
        }

        if let Some((symoff, nsyms, stroff, strsize)) = symtab {
            let mut symbols_cursor = Cursor::new(image_bytes);
            symbols_cursor
                .seek(SeekFrom::Start(symoff as u64))
                .map_err(|_| "invalid symbol table offset")?;
            for symbol in SymbolIter::new(&mut symbols_cursor, sections, nsyms, stroff, strsize, false, true) {
                if let Symbol::Defined { name: Some(symbol_name), entry, .. } = symbol {
                    exported_symbols.insert(symbol_name.to_string(), entry as u64 + slide);
                }
            }
        }

        Ok(Self {
            architecture: Architecture::Arm64,
            name,
            dynamic_libraries,
            exported_symbols,
            bindings,
            entry_point_pc,
            text_base: text_base.unwrap_or(0),
            last_segment_end,
            memory,
        })
    }
}
