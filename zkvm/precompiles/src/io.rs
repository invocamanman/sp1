#![allow(unused_unsafe)]
use crate::syscall_write;
use crate::{syscall_hint_len, syscall_hint_read};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::alloc::Layout;
use std::io::Write;

const FD_HINT: u32 = 4;
pub const FD_PUBLIC_VALUES: u32 = 3;

pub struct SyscallWriter {
    fd: u32,
}

impl std::io::Write for SyscallWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let nbytes = buf.len();
        let write_buf = buf.as_ptr();
        unsafe {
            syscall_write(self.fd, write_buf, nbytes);
        }
        Ok(nbytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub fn read_vec() -> Vec<u8> {
    let len = unsafe { syscall_hint_len() };
    // Round up to the nearest multiple of 4 so that the memory allocated is in whole words
    let capacity = (len + 3) / 4 * 4;

    // Allocate a buffer of the required length that is 4 byte aligned
    let layout = Layout::from_size_align(capacity, 4).expect("vec is too large");
    let ptr = unsafe { std::alloc::alloc(layout) };
    // SAFETY:
    // 1. `ptr` was allocated using alloc
    // 2. We assuume that the VM global allocator doesn't dealloc
    // 3/6. Size is correct from above
    // 4/5. Length is 0
    // 7. Layout::from_size_align already checks this
    let mut vec = unsafe { Vec::from_raw_parts(ptr, 0, capacity) };
    // Read the vec into uninitialized memory. The syscall assumes the memory is uninitialized,
    // which should be true because the allocator does not dealloc, so a new alloc should be fresh.
    unsafe {
        syscall_hint_read(ptr, len);
        vec.set_len(len);
    }
    vec
}

pub fn read<T: DeserializeOwned>() -> T {
    let vec = read_vec();
    println!("vec: {:?}", vec);
    unimplemented!();
    bincode::deserialize(&vec).expect("deserialization failed")
}

pub fn commit<T: Serialize>(value: &T) {
    let writer = SyscallWriter {
        fd: FD_PUBLIC_VALUES,
    };
    unimplemented!();
    bincode::serialize_into(writer, value).expect("serialization failed");
}

pub fn commit_slice(buf: &[u8]) {
    let mut my_writer = SyscallWriter {
        fd: FD_PUBLIC_VALUES,
    };
    my_writer.write_all(buf).unwrap();
}

pub fn hint<T: Serialize>(value: &T) {
    let writer = SyscallWriter { fd: FD_HINT };
    bincode::serialize_into(writer, value).expect("serialization failed");
}

pub fn hint_slice(buf: &[u8]) {
    let mut my_reader = SyscallWriter { fd: FD_HINT };
    my_reader.write_all(buf).unwrap();
}
