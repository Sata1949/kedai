//! 仅供历史 API 回归使用的线程内累计分配计数，不改变生产分配器。
//! 统计申请字节而非耗时/峰值，避免机器负载影响算法回归断言。

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static BYTES: Cell<Option<usize>> = const { Cell::new(None) };
}

struct CountingAllocator;

fn record(size: usize) {
    let _ = BYTES.try_with(|bytes| {
        if let Some(total) = bytes.get() {
            bytes.set(Some(total.saturating_add(size)));
        }
    });
}

// 安全性：原样转发 System 的指针、布局和大小；计数只访问无堆分配的 TLS Cell。
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record(size);
        unsafe { System.realloc(ptr, layout, size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

pub struct AllocationCount;

impl AllocationCount {
    /// 调用者须留在同一线程，且同一线程不得嵌套计数。
    pub fn start() -> Self {
        BYTES.with(|bytes| assert!(bytes.replace(Some(0)).is_none(), "分配计数不得嵌套"));
        Self
    }

    pub fn bytes(&self) -> usize {
        BYTES.with(|bytes| bytes.get().expect("分配计数已经启动"))
    }
}

impl Drop for AllocationCount {
    fn drop(&mut self) {
        BYTES.with(|bytes| bytes.set(None));
    }
}
