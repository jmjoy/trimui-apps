//! Shared GPU buffers for section meshes.
//!
//! Hundreds of small vertex and index buffers make wgpu track and validate
//! hundreds of resources every frame. Instead meshes live in a few large
//! pages; a draw only selects its range with a base vertex.

/// Vertices per page (16 bytes each: 4 MiB).
pub const PAGE_VERTICES: u32 = 256 * 1024;
/// Indices per page (2 bytes each: 1.5 MiB).
pub const PAGE_INDICES: u32 = 768 * 1024;

/// First-fit allocator over a range of units.
#[derive(Debug)]
pub struct FreeList {
    /// Sorted, non-overlapping, non-adjacent free spans (start, length).
    spans: Vec<(u32, u32)>,
    capacity: u32,
}
impl FreeList {
    pub fn new(capacity: u32) -> Self {
        Self {
            spans: vec![(0, capacity)],
            capacity,
        }
    }
    pub fn alloc(&mut self, len: u32) -> Option<u32> {
        if len == 0 {
            return Some(0);
        }
        let i = self.spans.iter().position(|&(_, l)| l >= len)?;
        let (start, l) = self.spans[i];
        if l == len {
            self.spans.remove(i);
        } else {
            self.spans[i] = (start + len, l - len);
        }
        Some(start)
    }
    pub fn free(&mut self, start: u32, len: u32) {
        if len == 0 {
            return;
        }
        let i = self.spans.partition_point(|&(s, _)| s < start);
        self.spans.insert(i, (start, len));
        // Merge with the following and preceding spans.
        if i + 1 < self.spans.len() && self.spans[i].0 + self.spans[i].1 == self.spans[i + 1].0 {
            self.spans[i].1 += self.spans[i + 1].1;
            self.spans.remove(i + 1);
        }
        if i > 0 && self.spans[i - 1].0 + self.spans[i - 1].1 == self.spans[i].0 {
            self.spans[i - 1].1 += self.spans[i].1;
            self.spans.remove(i);
        }
    }
    pub fn used(&self) -> u32 {
        self.capacity - self.spans.iter().map(|(_, l)| l).sum::<u32>()
    }
}

pub struct Page {
    pub vertices: wgpu::Buffer,
    pub indices: wgpu::Buffer,
    vfree: FreeList,
    ifree: FreeList,
}

/// Where a mesh lives: page, first vertex, first index (and the counts
/// needed to free it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    pub page: usize,
    pub base_vertex: u32,
    pub vertex_count: u32,
    pub first_index: u32,
    pub index_count: u32,
}

#[derive(Default)]
pub struct Arena {
    pub pages: Vec<Page>,
}

impl Arena {
    /// Copies a mesh into a page with room, creating one if needed.
    /// Index counts are padded to even numbers (4-byte copy alignment).
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vertices: &[u8],
        vertex_count: u32,
        indices: &[u16],
    ) -> Option<Slot> {
        let index_count = indices.len() as u32;
        let padded = index_count.next_multiple_of(2);
        if vertex_count > PAGE_VERTICES.min(u32::from(u16::MAX) + 1) || padded > PAGE_INDICES {
            return None;
        }
        let page = match self.pages.iter_mut().position(|p| {
            p.vfree.spans.iter().any(|&(_, l)| l >= vertex_count)
                && p.ifree.spans.iter().any(|&(_, l)| l >= padded)
        }) {
            Some(i) => i,
            None => {
                self.pages.push(Page {
                    vertices: device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("section vertex page"),
                        size: u64::from(PAGE_VERTICES) * 16,
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    }),
                    indices: device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("section index page"),
                        size: u64::from(PAGE_INDICES) * 2,
                        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    }),
                    vfree: FreeList::new(PAGE_VERTICES),
                    ifree: FreeList::new(PAGE_INDICES),
                });
                log::info!("Mesh arena: page {} created", self.pages.len());
                self.pages.len() - 1
            }
        };
        let p = &mut self.pages[page];
        let base_vertex = p.vfree.alloc(vertex_count)?;
        let Some(first_index) = p.ifree.alloc(padded) else {
            p.vfree.free(base_vertex, vertex_count);
            return None;
        };
        queue.write_buffer(&p.vertices, u64::from(base_vertex) * 16, vertices);
        if index_count == padded {
            queue.write_buffer(
                &p.indices,
                u64::from(first_index) * 2,
                bytemuck::cast_slice(indices),
            );
        } else {
            let mut data = indices.to_vec();
            data.push(0);
            queue.write_buffer(
                &p.indices,
                u64::from(first_index) * 2,
                bytemuck::cast_slice(&data),
            );
        }
        Some(Slot {
            page,
            base_vertex,
            vertex_count,
            first_index,
            index_count: padded,
        })
    }
    pub fn free(&mut self, slot: Slot) {
        if let Some(p) = self.pages.get_mut(slot.page) {
            p.vfree.free(slot.base_vertex, slot.vertex_count);
            p.ifree.free(slot.first_index, slot.index_count);
        }
    }
    /// (used vertex MiB, used index MiB, pages).
    pub fn usage(&self) -> (f32, f32, usize) {
        let v: u32 = self.pages.iter().map(|p| p.vfree.used()).sum();
        let i: u32 = self.pages.iter().map(|p| p.ifree.used()).sum();
        (
            v as f32 * 16.0 / 1_048_576.0,
            i as f32 * 2.0 / 1_048_576.0,
            self.pages.len(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_list_allocates_first_fit_and_merges() {
        let mut f = FreeList::new(100);
        let a = f.alloc(30).unwrap();
        let b = f.alloc(30).unwrap();
        let c = f.alloc(30).unwrap();
        assert_eq!((a, b, c), (0, 30, 60));
        assert!(f.alloc(20).is_none());
        assert_eq!(f.alloc(10), Some(90));
        f.free(b, 30);
        assert_eq!(f.used(), 70);
        assert_eq!(f.alloc(25), Some(30), "reuses the hole");
        f.free(a, 30);
        f.free(30, 25);
        // a and the remainder of b's hole merged into one span 0..60.
        assert_eq!(f.alloc(60), Some(0));
        f.free(0, 60);
        f.free(c, 30);
        f.free(90, 10);
        assert_eq!(f.used(), 0);
        assert_eq!(f.spans, [(0, 100)]);
        assert_eq!(f.alloc(0), Some(0));
    }
}
