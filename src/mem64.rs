use std::collections::BTreeMap;

use crate::mem::{SafeRead, SafeWrite};

pub type Guest64USize = u64;
pub type Guest64Addr = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub base: Guest64Addr,
    pub size: Guest64USize,
}

#[derive(Debug, Default)]
pub struct Mem64 {
    regions: BTreeMap<Guest64Addr, Vec<u8>>,
    allocations: BTreeMap<Guest64Addr, Guest64USize>,
    next_allocation: Guest64Addr,
}

impl Mem64 {
    pub fn new() -> Self {
        Self { next_allocation: 0x1_0000_0000, ..Self::default() }
    }

    pub fn map_zeroed(&mut self, base: Guest64Addr, size: Guest64USize) -> Result<(), &'static str> {
        let size_usize = usize::try_from(size).map_err(|_| "64-bit mapping is too large for this host")?;
        let end = base.checked_add(size).ok_or("64-bit mapping overflows")?;
        if size == 0 { return Ok(()); }
        if let Some((&previous_base, previous)) = self.regions.range(..=base).next_back() {
            let previous_end = previous_base.checked_add(previous.len() as u64).ok_or("mapping overflows")?;
            if previous_end > base { return Err("64-bit mapping overlaps an existing mapping"); }
        }
        if self.regions.range(base..).next().is_some_and(|(&next_base, _)| next_base < end) {
            return Err("64-bit mapping overlaps an existing mapping");
        }
        self.regions.insert(base, vec![0; size_usize]);
        Ok(())
    }

    pub fn write_bytes(&mut self, base: Guest64Addr, bytes: &[u8]) -> Result<(), &'static str> {
        self.slice_mut(base, bytes.len())?.copy_from_slice(bytes);
        Ok(())
    }

    pub fn fill_bytes(&mut self, base: Guest64Addr, value: u8, size: Guest64USize) -> Result<(), &'static str> {
        let size = usize::try_from(size).map_err(|_| "64-bit fill is too large for this host")?;
        self.slice_mut(base, size)?.fill(value);
        Ok(())
    }

    pub fn copy_bytes(&mut self, destination: Guest64Addr, source: Guest64Addr, size: Guest64USize) -> Result<(), &'static str> {
        let size = usize::try_from(size).map_err(|_| "64-bit copy is too large for this host")?;
        let bytes = self.slice(source, size)?.to_vec();
        self.slice_mut(destination, size)?.copy_from_slice(&bytes);
        Ok(())
    }

    pub fn cstr_len(&self, base: Guest64Addr, limit: Guest64USize) -> Result<Guest64USize, &'static str> {
        let limit = usize::try_from(limit).map_err(|_| "64-bit string limit is too large for this host")?;
        for length in 0..limit {
            if self.read_u8(base + length as u64)? == 0 {
                return Ok(length as u64);
            }
        }
        Err("64-bit string has no terminator within the safety limit")
    }

    pub fn read_bytes(&self, base: Guest64Addr, size: Guest64USize) -> Result<Vec<u8>, &'static str> {
        let size = usize::try_from(size).map_err(|_| "64-bit read is too large for this host")?;
        Ok(self.slice(base, size)?.to_vec())
    }

    pub fn allocation_size(&self, address: Guest64Addr) -> Option<Guest64USize> {
        self.allocations.get(&address).copied()
    }

    pub fn alloc_zeroed(&mut self, size: Guest64USize) -> Result<Guest64Addr, &'static str> {
        let size = size.max(16).checked_add(15).ok_or("allocation size overflows")? & !15;
        let mut base = self.next_allocation.max(0x1_0000_0000);
        loop {
            let end = base.checked_add(size).ok_or("allocation address overflows")?;
            let overlapping = self.regions.range(..end).next_back().and_then(|(&region_base, bytes)| {
                let region_end = region_base.checked_add(bytes.len() as u64)?;
                (region_end > base && region_base < end).then_some(region_end)
            });
            match overlapping {
                Some(region_end) => base = region_end.checked_add(15).ok_or("allocation address overflows")? & !15,
                None => break,
            }
        }
        self.map_zeroed(base, size)?;
        self.allocations.insert(base, size);
        self.next_allocation = base.checked_add(size).ok_or("allocation cursor overflows")?;
        Ok(base)
    }

    fn region_base(&self, addr: Guest64Addr, size: usize) -> Result<Guest64Addr, &'static str> {
        let (&base, bytes) = self.regions.range(..=addr).next_back().ok_or("64-bit memory access is unmapped")?;
        let offset = addr.checked_sub(base).ok_or("64-bit address underflow")?;
        let end = offset.checked_add(size as u64).ok_or("64-bit access overflows")?;
        if end > bytes.len() as u64 { return Err("64-bit memory access is out of bounds"); }
        Ok(base)
    }

    fn slice(&self, addr: Guest64Addr, size: usize) -> Result<&[u8], &'static str> {
        let base = self.region_base(addr, size)?;
        let offset = usize::try_from(addr - base).map_err(|_| "64-bit offset overflows host usize")?;
        Ok(&self.regions[&base][offset..offset + size])
    }

    fn slice_mut(&mut self, addr: Guest64Addr, size: usize) -> Result<&mut [u8], &'static str> {
        let base = self.region_base(addr, size)?;
        let offset = usize::try_from(addr - base).map_err(|_| "64-bit offset overflows host usize")?;
        Ok(&mut self.regions.get_mut(&base).unwrap()[offset..offset + size])
    }

    pub fn read<T: SafeRead + Copy>(&self, addr: Guest64Addr) -> Result<T, &'static str> {
        let size = std::mem::size_of::<T>();
        let source = self.slice(addr, size)?;
        let mut value = std::mem::MaybeUninit::<T>::uninit();
        unsafe {
            std::ptr::copy_nonoverlapping(source.as_ptr(), value.as_mut_ptr().cast(), size);
            Ok(value.assume_init())
        }
    }

    pub fn write<T: SafeWrite>(&mut self, addr: Guest64Addr, value: T) -> Result<(), &'static str> {
        let size = std::mem::size_of::<T>();
        let target = self.slice_mut(addr, size)?;
        unsafe { std::ptr::copy_nonoverlapping((&value as *const T).cast(), target.as_mut_ptr(), size) }
        Ok(())
    }

    pub fn read_u8(&self, addr: Guest64Addr) -> Result<u8, &'static str> { self.read(addr) }
    pub fn read_u16(&self, addr: Guest64Addr) -> Result<u16, &'static str> { self.read(addr) }
    pub fn read_u32(&self, addr: Guest64Addr) -> Result<u32, &'static str> { self.read(addr) }
    pub fn read_u64(&self, addr: Guest64Addr) -> Result<u64, &'static str> { self.read(addr) }
    pub fn read_u128(&self, addr: Guest64Addr) -> Result<[u64; 2], &'static str> { self.read(addr) }
    pub fn write_u8(&mut self, addr: Guest64Addr, value: u8) -> Result<(), &'static str> { self.write(addr, value) }
    pub fn write_u16(&mut self, addr: Guest64Addr, value: u16) -> Result<(), &'static str> { self.write(addr, value) }
    pub fn write_u32(&mut self, addr: Guest64Addr, value: u32) -> Result<(), &'static str> { self.write(addr, value) }
    pub fn write_u64(&mut self, addr: Guest64Addr, value: u64) -> Result<(), &'static str> { self.write(addr, value) }
    pub fn write_u128(&mut self, addr: Guest64Addr, value: [u64; 2]) -> Result<(), &'static str> { self.write(addr, value) }

    pub fn mapped_regions(&self) -> impl Iterator<Item = Region> + '_ {
        self.regions.iter().map(|(&base, bytes)| Region { base, size: bytes.len() as u64 })
    }
}

#[cfg(test)]
mod tests {
    use super::Mem64;

    #[test]
    fn allocations_skip_loaded_regions() {
        let mut mem = Mem64::new();
        mem.map_zeroed(0x1_0000_0000, 0x2000).unwrap();
        let allocation = mem.alloc_zeroed(0x100).unwrap();
        assert_eq!(allocation, 0x1_0000_2000);
    }

    #[test]
    fn accesses_are_checked_at_region_boundaries() {
        let mut mem = Mem64::new();
        mem.map_zeroed(0x1_0000_0000, 0x10).unwrap();
        assert!(mem.write_u64(0x1_0000_0008, 1).is_ok());
        assert!(mem.write_u64(0x1_0000_0009, 1).is_err());
        assert!(mem.read_u32(0x1_0000_000e).is_err());
    }
}
