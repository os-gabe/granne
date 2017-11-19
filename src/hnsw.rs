//
// concurrent, little waiting (X)
// mmap (X)
// build layer by layer (X)
// small size
// extenstible
// merge indexes?
// fast
//

use types::*;
use arrayvec::ArrayVec;
use std::collections::BinaryHeap;
use std::collections::HashSet;
use std::cmp::Ordering;
use std::iter;
use std::cmp;
use time;
use std::mem;
pub use ordered_float::NotNaN;

// Write and read
use std::fs::File;
use std::io::prelude::*;
use memmap::Mmap;
use revord::RevOrd;

// Threading
use std::sync::{Arc, RwLock};
use rayon::prelude::*;
use fnv::FnvHashSet;

const LEVELS: usize = 5;
const LEVEL_MULTIPLIER: usize = 12;

const MAX_NEIGHBORS: usize = 20;
const MAX_INDEX_SEARCH: usize = 500;
const MAX_SEARCH: usize = 800;

#[derive(Clone, Default, Debug)]
struct HnswNode {
    neighbors: ArrayVec<[usize; MAX_NEIGHBORS]>,
}

pub struct Config {
    num_levels: usize,
    level_multiplier: usize,
    max_search: usize,
}

pub struct HnswBuilder<'a> {
    levels: Vec<Vec<HnswNode>>,
    elements: &'a [Element],
}


pub struct Hnsw<'a> {
    levels: Vec<&'a [HnswNode]>,
    elements: &'a [Element]
}


impl<'a> HnswBuilder<'a> {
    pub fn new(elements: &'a [Element]) -> Self {
        HnswBuilder {
            levels: Vec::new(),
            elements: elements,
        }
    }


    pub fn save_to_disk(self: &Self, path: &str) {

        let mut file = File::create(path).unwrap();

        self.write(&mut file);
    }


    pub fn write<T: Write>(self: &Self, buffer: &mut T) {
        let num_nodes = self.levels.iter().map(|level| level.len()).sum();
        let num_levels = self.levels.len();
        let level_counts = self.levels.iter().map(|level| level.len());

        let mut usize_data = vec![num_nodes, num_levels];
        usize_data.extend(level_counts);

        let data = unsafe {
            ::std::slice::from_raw_parts(
                usize_data.as_ptr() as *const u8,
                usize_data.len() * ::std::mem::size_of::<usize>())
        };

        buffer.write(data);

        for level in &self.levels {

            let data = unsafe {
                ::std::slice::from_raw_parts(
                    level.as_ptr() as *const u8,
                    level.len() * ::std::mem::size_of::<HnswNode>())
            };

            buffer.write(data);
        }
    }


    pub fn build_index(&mut self) {
        self.levels.push(vec![HnswNode::default()]);

        let mut num_elements = 1;
        for level in 1..LEVELS {
            num_elements *= LEVEL_MULTIPLIER;
            num_elements = cmp::min(num_elements, self.elements.len());

            let mut new_layer = 
                Self::build_layer(&self.levels[..], &self.elements[..num_elements]);

            self.levels.push(new_layer);
        }
    }


    fn build_layer(layers: &[Vec<HnswNode>],
                   elements: &[Element]) -> Vec<HnswNode> {

        println!("Building layer {} with {} vectors", layers.len(), elements.len());

        // copy layer above
        let mut layer = Vec::with_capacity(elements.len());
        layer.extend_from_slice(layers.last().unwrap());
        layer.resize(elements.len(), HnswNode::default());

        {
            let already_inserted = layers.last().unwrap().len();

            // create RwLocks for underlying nodes
            let layer: Vec<RwLock<&mut HnswNode>> =
                layer.iter_mut()
                .map(|node| RwLock::new(node))
                .collect();

            // insert elements, skipping already inserted
            elements
                .par_iter()
                .enumerate()
                .skip(already_inserted)
                .for_each(
                    |(idx, element)| {
                        Self::insert_element(layers,
                                             &layer,
                                             elements,
                                             idx);
                    });
        }

        layer
    }


    fn insert_element(layers: &[Vec<HnswNode>],
                      layer: &Vec<RwLock<&mut HnswNode>>,
                      elements: &[Element],
                      idx: usize) {

        let element = &elements[idx];
        let entrypoint = Self::find_entrypoint(layers,
                                               element,
                                               elements);

        let neighbors = Self::search_for_neighbors_index(&layer[..],
                                                         entrypoint,
                                                         elements,
                                                         element,
                                                         MAX_INDEX_SEARCH,
                                                         MAX_NEIGHBORS);

        for neighbor in neighbors.into_iter().filter(|&n| n != idx) {
            // can be done directly since layer[idx].neighbors is empty
            Self::connect_nodes(&layer[idx], elements, idx, neighbor);

            // find a more clever way to decide when to add this edge
            Self::connect_nodes(&layer[neighbor], elements, neighbor, idx);
        }
    }


    fn search_for_neighbors_index(layer: &[RwLock<&mut HnswNode>],
                                  entrypoint: usize,
                                  elements: &[Element],
                                  goal: &Element,
                                  max_search: usize,
                                  max_neighbors: usize) -> Vec<usize> {

        let mut res = MaxSizeHeap::new(max_neighbors);
        let mut pq: BinaryHeap<RevOrd<_>> = BinaryHeap::new();
        let mut visited = HashSet::new();

        pq.push(RevOrd(
            (dist(&elements[entrypoint], &goal), entrypoint)
        ));

        visited.insert(entrypoint);

        for _ in 0..max_search {

            if let Some(RevOrd {0: (d, idx)} ) = pq.pop() {
                res.push((d, idx));

                let node = layer[idx].read().unwrap();

                for &neighbor_idx in &node.neighbors {
                    if visited.insert(neighbor_idx) {
                        let distance = dist(&elements[neighbor_idx], &goal);
                        pq.push(RevOrd((distance, neighbor_idx)));
                    }
                }

            } else {
                break;
            }
        }

        return res.heap.into_vec().into_iter().map(|(_, idx)| idx).collect();
    }


    fn connect_nodes(node: &RwLock<&mut HnswNode>,
                     elements: &[Element],
                     i: usize,
                     j: usize) -> bool
    {
        // Write Lock!
        let mut node = node.write().unwrap();

        if node.neighbors.len() < MAX_NEIGHBORS {
            node.neighbors.push(j);
            return true;
        } else {
            let current_distance =
                dist(&elements[i], &elements[j]);

            if let Some((k, max_dist)) = node.neighbors
                .iter()
                .map(|&k| dist(&elements[i], &elements[k]))
                .enumerate()
                .max()
            {
                if current_distance < NotNaN::new(2.0f32).unwrap() * max_dist {
                    node.neighbors[k] = j;
                    return true;
                }
            }
        }

        return false;
    }


    fn find_entrypoint(layers: &[Vec<HnswNode>],
                       element: &Element,
                       elements: &[Element]) -> usize {

        let mut entrypoint = 0;
        for layer in layers {
            let res = search_for_neighbors(
                &layer,
                entrypoint,
                &elements,
                &element,
                MAX_INDEX_SEARCH,
                1usize);

            entrypoint = res.first().unwrap().clone();
        }

        entrypoint
    }
}


impl<'a> Hnsw<'a> {

    pub fn load(buffer: &'a [u8], elements: &'a [Element]) -> Self {

        let offset = 0 * ::std::mem::size_of::<usize>();
        let num_nodes = &buffer[offset] as *const u8 as *const usize;

        let offset = 1 * ::std::mem::size_of::<usize>();
        let num_levels = &buffer[offset] as *const u8 as *const usize;

        let offset = 2 * ::std::mem::size_of::<usize>();

        let level_counts: &[usize] = unsafe {
            ::std::slice::from_raw_parts(
                &buffer[offset] as *const u8 as *const usize,
                *num_levels
        )};

        let offset = (2 + level_counts.len()) * ::std::mem::size_of::<usize>();

        let nodes: &[HnswNode] = unsafe {
            ::std::slice::from_raw_parts(
                &buffer[offset] as *const u8 as *const HnswNode,
                *num_nodes
            )
        };

        let mut levels = Vec::new();

        let mut start = 0;
        for &level_count in level_counts {
            let end = start + level_count;
            let level = &nodes[start..end];
            levels.push(level);
            start = end;
        }

        assert!(levels.last().unwrap().len() <= elements.len());

        Self {
            levels: levels,
            elements: elements,
        }

    }


    pub fn search(&self, element: &Element) -> Vec<(usize, f32)> {

        let entrypoint = Self::find_entrypoint(&self.levels[..LEVELS-1],
                                               element,
                                               &self.elements);

        search_for_neighbors(
            &self.levels[LEVELS-1],
            entrypoint,
            &self.elements,
            element,
            MAX_SEARCH,
            MAX_NEIGHBORS)
            .iter()
            .map(|&i| (i, dist(&self.elements[i], element).into_inner())).collect()
    }


    fn find_entrypoint(layers: &[&[HnswNode]],
                       element: &Element,
                       elements: &[Element]) -> usize {

        let mut entrypoint = 0;
        for layer in layers {
            let res = search_for_neighbors(
                &layer,
                entrypoint,
                &elements,
                &element,
                MAX_SEARCH,
                1usize);

            entrypoint = res.first().unwrap().clone();
        }

        entrypoint
    }
}


fn search_for_neighbors(layer: &[HnswNode],
                        entrypoint: usize,
                        elements: &[Element],
                        goal: &Element,
                        max_search: usize,
                        max_neighbors: usize) -> Vec<usize> {

    let mut res = MaxSizeHeap::new(max_neighbors);
    let mut pq: BinaryHeap<RevOrd<_>> = BinaryHeap::new();
    let mut visited = HashSet::new();

    pq.push(RevOrd(
        (dist(&elements[entrypoint], &goal), entrypoint)
    ));

    visited.insert(entrypoint);

    for _ in 0..max_search {

        if let Some(RevOrd {0: (d, idx)} ) = pq.pop() {
            res.push((d, idx));

            let node = &layer[idx];

            for &neighbor_idx in &node.neighbors {
                if visited.insert(neighbor_idx) {
                    let distance = dist(&elements[neighbor_idx], &goal);
                    pq.push(RevOrd((distance, neighbor_idx)));
                }
            }

        } else {
            break;
        }
    }

    return res.heap.into_sorted_vec().into_iter().map(|(_, idx)| idx).collect();
}


struct MaxSizeHeap<T> {
    heap: BinaryHeap<T>,
    max_size: usize
}

impl<T: Ord> MaxSizeHeap<T> {

    pub fn new(max_size: usize) -> Self {
        MaxSizeHeap {
            heap: BinaryHeap::with_capacity(max_size),
            max_size: max_size
        }
    }

    pub fn push(self: &mut Self, element: T) {
        if self.heap.len() < self.max_size {
            self.heap.push(element);
        }
        else if element < *self.heap.peek().unwrap() {
            if self.heap.len() >= self.max_size {
                self.heap.pop();
            }

            self.heap.push(element);
        }
    }

    pub fn peek(self: &Self) -> Option<&T> {
        self.heap.peek()
    }

    pub fn len(self: &Self) -> usize {
        self.heap.len()
    }
}


mod tests {
    use super::*;

    #[test]
    fn test_hnsw_node_size()
    {
        assert!((MAX_NEIGHBORS) * mem::size_of::<usize>() < mem::size_of::<HnswNode>());
    }

    #[test]
    fn test_hnsw()
    {

    }
}