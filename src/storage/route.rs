//! Stock's routing graph, read from its map files: the roads ahead of the rider, with the elevation of each node
//! (climbs without a course: `climbs_ahead`). Format reverse engineered for interoperability (STORAGE_AND_MAPS.md
//! 4.4, 4.7 and 4.9 in the bryton repo, the reader `RouteData_DecodeNodeTile` and the nanopb tables behind it).
//!
//! - **Where:** `\Map\Update\<Region>\9-<x>-<y>.dat`, else `\Map\MapData\<Region>\9-<x>-<y>.dat`, for the z9 tile
//!   (x, y) that holds the z14 tile; any region folder, except the `Topo`, `Contour` and `Route` ones stock skips.
//! - **Container** ([`Dat`]): a 0x12-byte header whose u32 at 0x0e is the routing section; there 20 u32 zoom offsets
//!   (only 14 is used), a 100-byte zoom header ("BRYTON", the range of tiles, the index entry and length sizes, where
//!   the data starts), an index of one entry per tile (column-major, all ones: no tile), and each tile a length
//!   then one gzip stream ([`crate::inflate`]).
//! - **Tile** (protobuf, [`Graph::decode`]): base latitude and longitude (sint32, 1e-7 degrees, the centre of the
//!   z14 tile), `deltaBits` (20), `numOfTransport` (2), the nodes and the ways. A node: its key, its elevation
//!   (`field / 5 - 500` metres), the ids of its ways (packed), and its edges (the neighbour's key and an attribute
//!   byte; every edge is listed at both ends, across tiles too).
//! - **Keys, corrected** (this module's reader, from 451,537 nodes of one real file, 2026-10-26): a key packs two
//!   halves of `deltaBits` bits, latitude high, longitude low, and each half is **sign and magnitude** (the top bit
//!   is the sign, the other 19 the distance from the base), not two's complement as the first reading of stock's
//!   code had it. Read so, every node but one lies in its own z14 tile, and an edge's far end is in the tile of its
//!   position; read as two's complement three nodes in four are in the wrong tile and 1.5 % of the edges are
//!   kilometres long. `tools/route_pack.py info` checks a file this way.
//! - **Memory:** a [`Graph`] holds a few tiles at once (nodes [`NODES`], edges [`EDGES`], tiles [`TILES`]), each
//!   tile's nodes sorted by position for a binary search, the neighbours kept as positions (a neighbour in a tile not
//!   loaded yet is read when the trace needs it). A tile that does not fit is refused ([`Error::Full`]) and the
//!   caller starts again from an empty graph; the inflated tile (median 16 KB, largest seen 150 KB) goes through a
//!   [`Scratch`] buffer that main.rs takes from the map's SDRAM.
//! - **Never panics:** a damaged container, gzip or protobuf gives an [`Error`] (every index checked, every loop
//!   bounded by the data or a count); sim/fuzz.rs feeds it damaged files.

use crate::fat::{self, Disk, Fat, File, Seek};
use crate::ign::Bytes;
use crate::inflate::{Inflater, Scratch};
use crate::text::Line;

/// Nodes the graph holds at most (20 bytes each). A real tile has 445 at the median and 2868 at most in the
/// mountain region measured; a tile that does not fit is skipped.
pub const NODES: usize = 2560;
/// Edges the graph holds at most (9 bytes each): two or three a node.
pub const EDGES: usize = 5120;
/// Tiles remembered at most, the ones with no data too.
pub const TILES: usize = 16;
/// The elevation of a node that has none.
pub const NO_ELEV: i16 = i16::MIN;
/// The words of buffer a tile is inflated into (96 KB: the inflated tile of a full graph, about 36 bytes a node);
/// main.rs takes them from the map's SDRAM, behind the basemap's (link.x MAP).
pub const SCRATCH_WORDS: usize = 48 * 1024;
/// Edges kept for a node at most (the real maximum seen is 9).
const MAX_EDGES: usize = 15;
/// Tiles a z9 file spans a side: 2^(14 - 9).
const Z9_SHIFT: u32 = 5;

/// Why a tile or a file could not be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Not a tile this reader takes: damaged, or a format it does not know.
    Bad,
    /// The graph has no room for the tile.
    Full,
}

/// A node of the graph.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Node {
    /// Latitude, degrees x 1e-7.
    pub lat: i32,
    /// Longitude, degrees x 1e-7.
    pub lon: i32,
    /// Elevation, m ([`NO_ELEV`]: none).
    pub elev: i16,
    /// The ways through it, each folded to 16 bits (0: none); only the first two are kept.
    pub ways: [u16; 2],
    first: u16,
    n_edges: u8,
}

impl Node {
    const ZERO: Node = Node { lat: 0, lon: 0, elev: 0, ways: [0; 2], first: 0, n_edges: 0 };

    /// The two nodes are on a common way; Some(that way's folded id).
    pub fn common_way(&self, other: &Node) -> Option<u16> {
        self.ways.iter().copied().find(|&w| w != 0 && other.ways.contains(&w))
    }

    /// The node is on way `w`.
    pub fn on_way(&self, w: u16) -> bool {
        w != 0 && self.ways.contains(&w)
    }
}

/// What the graph knows of a z14 tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileState {
    /// Not asked for.
    Unknown,
    /// Loaded.
    Loaded,
    /// Asked for and not there (not in the file, damaged, too big).
    Absent,
}

#[derive(Clone, Copy)]
struct Slot {
    x: u16,
    y: u16,
    start: u16,
    end: u16,
    loaded: bool,
    /// The nodes' box: lowest and highest latitude, lowest and highest longitude.
    bbox: [i32; 4],
}

impl Slot {
    const ZERO: Slot = Slot { x: 0, y: 0, start: 0, end: 0, loaded: false, bbox: [0; 4] };
}

/// The nodes of the loaded tiles. About 100 KB: main.rs keeps it in AXI SRAM (`climbs_ahead::Ahead` holds it).
pub struct Graph {
    nodes: [Node; NODES],
    n: usize,
    to: [(i32, i32); EDGES],
    attr: [u8; EDGES],
    n_edges: usize,
    slots: [Slot; TILES],
    n_slots: usize,
}

impl Default for Graph {
    fn default() -> Self {
        Self::new()
    }
}

/// A protobuf field's value.
enum Val {
    Varint(u64),
    /// A length-delimited field: its bytes' place in the buffer.
    Bytes(usize, usize),
    Other,
}

/// Fields of a protobuf message in `s[pos..end]`.
struct Reader<'a, 'b> {
    s: &'a Scratch<'b>,
    pos: usize,
    end: usize,
}

impl Reader<'_, '_> {
    fn varint(&mut self) -> Result<u64, Error> {
        let mut v = 0u64;
        for shift in (0..70).step_by(7) {
            if self.pos >= self.end {
                return Err(Error::Bad);
            }
            let b = self.s.get(self.pos).ok_or(Error::Bad)?;
            self.pos += 1;
            if shift < 64 {
                v |= u64::from(b & 0x7f) << shift;
            }
            if b < 0x80 {
                return Ok(v);
            }
        }
        Err(Error::Bad)
    }

    /// The next field: its number and value; None at the end.
    fn next(&mut self) -> Result<Option<(u32, Val)>, Error> {
        if self.pos >= self.end {
            return Ok(None);
        }
        let tag = self.varint()?;
        let skip = |r: &mut Self, n: usize| {
            r.pos = r.pos.checked_add(n).filter(|&p| p <= r.end).ok_or(Error::Bad)?;
            Ok::<_, Error>(Val::Other)
        };
        let v = match tag & 7 {
            0 => Val::Varint(self.varint()?),
            1 => skip(self, 8)?,
            5 => skip(self, 4)?,
            2 => {
                let n = usize::try_from(self.varint()?).map_err(|_| Error::Bad)?;
                let start = self.pos;
                skip(self, n)?;
                Val::Bytes(start, self.pos)
            }
            _ => return Err(Error::Bad),
        };
        Ok(Some(((tag >> 3) as u32, v)))
    }

    fn sub(&self, a: usize, b: usize) -> Self {
        Reader { s: self.s, pos: a, end: b }
    }
}

/// A heapsort of nodes by position, in place (core's `sort_unstable` is 3.5 KB of image for this one use), that
/// can stop between two sifts and go on: where it is.
#[derive(Clone, Copy, Default)]
pub struct Sort {
    phase: u8,
    i: usize,
    end: usize,
}

impl Sort {
    /// Sorts `v` for at most `steps` sifts (each a dozen compares at most); true when it is sorted. Call again with
    /// the same `v` until true.
    pub fn step(&mut self, v: &mut [Node], mut steps: usize) -> bool {
        fn down(v: &mut [Node], mut root: usize, end: usize) {
            let key = |v: &[Node], i: usize| v.get(i).map(|n| (n.lat, n.lon));
            loop {
                let mut child = 2 * root + 1;
                if child >= end {
                    return;
                }
                if child + 1 < end && key(v, child) < key(v, child + 1) {
                    child += 1;
                }
                if key(v, root) >= key(v, child) {
                    return;
                }
                v.swap(root, child); // both below `end`, which is at most v.len()
                root = child;
            }
        }
        let n = v.len();
        loop {
            match self.phase {
                0 => (self.i, self.phase) = (n / 2, 1),
                1 if self.i == 0 => (self.end, self.phase) = (n, 2),
                1 => {
                    self.i -= 1;
                    down(v, self.i, n);
                    steps = steps.saturating_sub(1);
                }
                2 if self.end <= 1 => self.phase = 3,
                2 => {
                    self.end -= 1;
                    v.swap(0, self.end); // end < n
                    down(v, 0, self.end);
                    steps = steps.saturating_sub(1);
                }
                _ => return true,
            }
            if steps == 0 {
                return self.phase == 3;
            }
        }
    }
}

/// How much of a tile one call of [`Maps::step`] does at most (the caller also watches the clock between calls).
#[derive(Clone, Copy, Debug)]
pub struct Slice {
    /// Bytes inflated (a copy of up to 258 more may follow).
    pub inflate: usize,
    /// Nodes decoded.
    pub nodes: usize,
    /// Sifts of the sort.
    pub sort: usize,
}

impl Slice {
    /// The slice of a main loop pass: about a millisecond or two on the unit each (src/sim/ counts the operations).
    pub const PASS: Slice = Slice { inflate: 1024, nodes: 48, sort: 200 };
    /// The whole tile in one call.
    #[cfg(test)]
    pub const ALL: Slice = Slice { inflate: usize::MAX, nodes: usize::MAX, sort: usize::MAX };
}

/// A tile's header: the base, deltaBits and the transport count; `seen` has bit 0 to 2 set for each of the first three.
#[derive(Clone, Copy)]
struct Hdr {
    lat0: i64,
    lon0: i64,
    bits: u32,
    transport: u64,
    seen: u8,
}

impl Hdr {
    fn take(&mut self, f: u32, v: &Val) {
        match (f, v) {
            (1, Val::Varint(z)) => (self.lat0, self.seen) = (zigzag(*z), self.seen | 1),
            (2, Val::Varint(z)) => (self.lon0, self.seen) = (zigzag(*z), self.seen | 2),
            (3, Val::Varint(b)) => (self.bits, self.seen) = (u32::try_from(*b).unwrap_or(0), self.seen | 4),
            (4, Val::Varint(t)) => self.transport = *t,
            _ => {}
        }
    }

    /// The header of the whole message, wherever its fields are.
    fn scan(buf: &Scratch<'_>, len: usize) -> Result<Hdr, Error> {
        let mut h = Hdr { lat0: 0, lon0: 0, bits: 0, transport: 2, seen: 0 };
        let mut top = Reader { s: buf, pos: 0, end: len };
        while let Some((f, v)) = top.next()? {
            h.take(f, &v);
        }
        Ok(h)
    }

    fn valid(&self) -> bool {
        (2..=31).contains(&self.bits) && self.transport == 2
    }

    /// A key's halves: sign and magnitude (module doc), added to the base; None outside the world.
    fn place(&self, key: u64) -> Option<(i32, i32)> {
        let bits = self.bits;
        if key >> (2 * bits) != 0 {
            return None;
        }
        let half = |h: u64| {
            let m = (h & ((1 << (bits - 1)) - 1)) as i64;
            if h >> (bits - 1) & 1 == 1 { -m } else { m }
        };
        let mask = (1u64 << bits) - 1;
        let (lat, lon) = (self.lat0 + half(key >> bits & mask), self.lon0 + half(key & mask));
        ((-900_000_000..=900_000_000).contains(&lat) && (-1_800_000_000..=1_800_000_000).contains(&lon))
            .then_some((lat as i32, lon as i32))
    }
}

fn zigzag(v: u64) -> i64 {
    let v = v as u32;
    i64::from((v >> 1) as i32 ^ -((v & 1) as i32))
}

/// A way's id folded to 16 bits for comparing (never 0).
fn fold(id: u64) -> u16 {
    let x = (id ^ id >> 32) as u32;
    ((x ^ x >> 16) as u16).max(1)
}

impl Graph {
    /// An empty graph.
    pub const fn new() -> Graph {
        Graph {
            nodes: [Node::ZERO; NODES],
            n: 0,
            to: [(0, 0); EDGES],
            attr: [0; EDGES],
            n_edges: 0,
            slots: [Slot::ZERO; TILES],
            n_slots: 0,
        }
    }

    /// Forgets everything.
    pub fn clear(&mut self) {
        (self.n, self.n_edges, self.n_slots) = (0, 0, 0);
    }

    /// Nodes held.
    pub fn len(&self) -> usize {
        self.n
    }

    /// No node held.
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// The nodes held, tile after tile.
    pub fn nodes(&self) -> &[Node] {
        self.nodes.get(..self.n).unwrap_or(&[])
    }

    /// What is known of z14 tile (`x`, `y`).
    pub fn state(&self, x: u32, y: u32) -> TileState {
        match self.slots.iter().take(self.n_slots).find(|s| u32::from(s.x) == x && u32::from(s.y) == y) {
            None => TileState::Unknown,
            Some(s) if s.loaded => TileState::Loaded,
            Some(_) => TileState::Absent,
        }
    }

    /// Notes that z14 tile (`x`, `y`) has no data to load; false when the tile table is full.
    pub fn mark_absent(&mut self, x: u32, y: u32) -> bool {
        let (Ok(x), Ok(y)) = (u16::try_from(x), u16::try_from(y)) else { return true };
        match self.slots.get_mut(self.n_slots) {
            Some(s) => {
                *s = Slot { x, y, ..Slot::ZERO };
                self.n_slots += 1;
                true
            }
            None => false,
        }
    }

    /// The node at (`lat`, `lon`).
    pub fn find(&self, lat: i32, lon: i32) -> Option<usize> {
        for s in self.slots.iter().take(self.n_slots).filter(|s| s.loaded) {
            let b = s.bbox;
            if lat < b[0] || lat > b[1] || lon < b[2] || lon > b[3] {
                continue;
            }
            let r = self.nodes.get(usize::from(s.start)..usize::from(s.end))?;
            if let Ok(i) = r.binary_search_by(|n| (n.lat, n.lon).cmp(&(lat, lon))) {
                return Some(usize::from(s.start) + i);
            }
        }
        None
    }

    /// Node `i`.
    pub fn node(&self, i: usize) -> Option<&Node> {
        self.nodes.get(..self.n)?.get(i)
    }

    /// The edges of node `i`: the neighbour's position and the attribute byte.
    pub fn edges(&self, i: usize) -> impl Iterator<Item = ((i32, i32), u8)> + '_ {
        let (a, n) = self.node(i).map_or((0, 0), |n| (usize::from(n.first), usize::from(n.n_edges)));
        let to = self.to.get(a..a + n).unwrap_or(&[]);
        let attr = self.attr.get(a..a + n).unwrap_or(&[]);
        to.iter().copied().zip(attr.iter().copied())
    }

    /// Starts tile (`x`, `y`)'s nodes: `add_node` until `end_tile`. Err(Full) when the tile table is full.
    pub(crate) fn begin_tile(&mut self, x: u32, y: u32) -> Result<(), Error> {
        let (Ok(x), Ok(y)) = (u16::try_from(x), u16::try_from(y)) else { return Err(Error::Bad) };
        let start = u16::try_from(self.n).map_err(|_| Error::Full)?;
        let s = self.slots.get_mut(self.n_slots).ok_or(Error::Full)?;
        *s = Slot { x, y, start, end: start, loaded: false, bbox: [i32::MAX, i32::MIN, i32::MAX, i32::MIN] };
        Ok(())
    }

    /// Adds a node to the tile being built, with its edges (position, attribute).
    pub(crate) fn add_node(
        &mut self,
        (lat, lon, elev): (i32, i32, i16),
        ways: [u16; 2],
        edges: &[(i32, i32, u8)],
    ) -> Result<(), Error> {
        let edges = edges.get(..edges.len().min(MAX_EDGES)).unwrap_or(&[]);
        if self.n >= NODES || self.n_edges + edges.len() > EDGES {
            return Err(Error::Full);
        }
        let first = u16::try_from(self.n_edges).map_err(|_| Error::Full)?;
        for (k, &(la, lo, at)) in edges.iter().enumerate() {
            if let (Some(t), Some(a)) = (self.to.get_mut(self.n_edges + k), self.attr.get_mut(self.n_edges + k)) {
                (*t, *a) = ((la, lo), at);
            }
        }
        let node = Node { lat, lon, elev, ways, first, n_edges: edges.len() as u8 };
        *self.nodes.get_mut(self.n).ok_or(Error::Full)? = node;
        (self.n, self.n_edges) = (self.n + 1, self.n_edges + edges.len());
        if let Some(s) = self.slots.get_mut(self.n_slots) {
            s.end = self.n as u16;
            s.bbox = [s.bbox[0].min(lat), s.bbox[1].max(lat), s.bbox[2].min(lon), s.bbox[3].max(lon)];
        }
        Ok(())
    }

    /// Ends the tile: its nodes sorted by position, the tile loaded. With `ok` false, everything it added is dropped.
    pub(crate) fn end_tile(&mut self, ok: bool) {
        if ok {
            let mut sort = Sort::default();
            self.sort_tile(&mut sort, usize::MAX);
        }
        self.finish_tile(ok);
    }

    /// Sorts the tile being built, `steps` sifts at most; true when done (then [`Graph::finish_tile`]).
    pub(crate) fn sort_tile(&mut self, sort: &mut Sort, steps: usize) -> bool {
        let Some(s) = self.slots.get(self.n_slots).copied() else { return true };
        match self.nodes.get_mut(usize::from(s.start)..usize::from(s.end)) {
            Some(r) => sort.step(r, steps),
            None => true,
        }
    }

    /// Closes the tile being built: loaded (its nodes sorted by [`Graph::sort_tile`]), or with `ok` false dropped
    /// with its nodes and edges.
    pub(crate) fn finish_tile(&mut self, ok: bool) {
        let Some(s) = self.slots.get_mut(self.n_slots).copied() else { return };
        if !ok {
            // the nodes it added go, and their edges (from the first one's first edge)
            let start = usize::from(s.start);
            if start < self.n {
                let first = self.nodes.get(start).map_or(self.n_edges, |n| usize::from(n.first));
                (self.n, self.n_edges) = (start, first.min(self.n_edges));
            }
            return;
        }
        if let Some(slot) = self.slots.get_mut(self.n_slots) {
            slot.loaded = true;
        }
        self.n_slots += 1;
    }

    /// Decodes z14 tile (`x`, `y`)'s protobuf, `buf[..len]`, into the graph. On an error the graph is as it was.
    pub fn decode(&mut self, x: u32, y: u32, buf: &Scratch<'_>, len: usize) -> Result<usize, Error> {
        self.begin_tile(x, y)?;
        let mut st = Dec::new();
        let r = loop {
            match self.decode_step(buf, len, &mut st, usize::MAX) {
                Ok(Some(n)) => break Ok(n),
                Ok(None) => {}
                Err(e) => break Err(e),
            }
        };
        self.end_tile(r.is_ok());
        r
    }

    /// Decodes on from where `st` is, at most `max` nodes; Some(nodes added in all) at the end of the message.
    /// The tile must have been begun; on an error the caller drops it ([`Graph::finish_tile`]).
    pub(crate) fn decode_step(
        &mut self,
        buf: &Scratch<'_>,
        len: usize,
        st: &mut Dec,
        max: usize,
    ) -> Result<Option<usize>, Error> {
        let len = len.min(buf.len());
        // One pass: in every tile of a real file (1014 of 1014) the base, deltaBits and transport come before the
        // first node; a tile that has them later, or none, is scanned for them first (the slow path)
        let mut top = Reader { s: buf, pos: st.pos, end: len };
        let mut added = 0;
        loop {
            st.pos = top.pos;
            if added >= max {
                return Ok(None);
            }
            let Some((f, v)) = top.next()? else { break };
            if st.ready || f != 5 {
                if !st.ready {
                    st.h.take(f, &v);
                }
                if f != 5 {
                    continue;
                }
            } else if st.h.seen & 7 != 7 {
                st.h = Hdr::scan(buf, len)?;
            }
            if !st.ready {
                if !st.h.valid() {
                    return Err(Error::Bad);
                }
                st.ready = true;
            }
            let h = st.h;
            let Val::Bytes(a, b) = v else { continue };
            let mut r = top.sub(a, b);
            let (mut key, mut elev, mut ways, mut edges) = (None, NO_ELEV, [0u16; 2], [(0i32, 0i32, 0u8); MAX_EDGES]);
            let mut n_edges = 0;
            while let Some((f, v)) = r.next()? {
                match (f, v) {
                    (1, Val::Varint(k)) => key = h.place(k),
                    (2, Val::Varint(e)) => elev = ((e / 5) as i64 - 500).clamp(-32_000, 32_000) as i16,
                    (3, Val::Bytes(a, b)) => {
                        let mut w = top.sub(a, b);
                        let mut k = 0;
                        while w.pos < w.end {
                            let id = w.varint()?;
                            if let Some(slot) = ways.get_mut(k) {
                                *slot = fold(id);
                                k += 1;
                            }
                        }
                    }
                    (4, Val::Bytes(a, b)) => {
                        let mut e = top.sub(a, b);
                        let (mut to, mut attr) = (None, 0u8);
                        while let Some((f, v)) = e.next()? {
                            match (f, v) {
                                (1, Val::Varint(k)) => to = h.place(k),
                                (2, Val::Varint(a)) => attr = a as u8,
                                _ => {}
                            }
                        }
                        if let (Some((la, lo)), Some(slot)) = (to, edges.get_mut(n_edges)) {
                            *slot = (la, lo, attr);
                            n_edges += 1;
                        }
                    }
                    _ => {}
                }
            }
            let Some((lat, lon)) = key else { continue };
            self.add_node((lat, lon, elev), ways, edges.get(..n_edges).unwrap_or(&[]))?;
            st.nodes += 1;
            added += 1;
        }
        if !st.ready {
            if st.h.seen & 7 != 7 {
                st.h = Hdr::scan(buf, len)?;
            }
            if !st.h.valid() {
                return Err(Error::Bad);
            }
        }
        Ok(Some(st.nodes))
    }
}

/// Where the protobuf of a tile is read: the next field's place, the header so far, the nodes added.
pub(crate) struct Dec {
    pos: usize,
    h: Hdr,
    ready: bool,
    nodes: usize,
}

impl Dec {
    /// All zero (a static of it stays in .bss); [`Dec::new`] is the real start.
    const ZERO: Dec =
        Dec { pos: 0, h: Hdr { lat0: 0, lon0: 0, bits: 0, transport: 0, seen: 0 }, ready: false, nodes: 0 };

    /// At the start of a message.
    pub(crate) const fn new() -> Dec {
        Dec { pos: 0, h: Hdr { lat0: 0, lon0: 0, bits: 0, transport: 2, seen: 0 }, ready: false, nodes: 0 }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The container
// ---------------------------------------------------------------------------------------------------------------

/// A map file's routing section at zoom 14.
#[derive(Clone, Copy, Debug)]
pub struct Dat {
    file: File,
    zb: u32,
    range: [u32; 4],
    idx: u8,
    len: u8,
    data: u32,
}

impl Dat {
    const ZERO: Dat =
        Dat { file: File { cluster: 0, size: 0, dir: false }, zb: 0, range: [0; 4], idx: 0, len: 0, data: 0 };
}

impl Dat {
    /// The routing section of `file`; None when it has none (no section 1, no zoom 14, not "BRYTON", ranges that
    /// cannot be: this file is not used).
    pub fn open(fat: &mut Fat, d: &mut impl Disk, file: File) -> Result<Option<Dat>, fat::Error> {
        let mut head = [0u8; 0x12];
        if fat.read_at(d, &file, 0, &mut head)? != head.len() {
            return Ok(None);
        }
        let sec = crate::storage::bytes::le32(&head, 0x0e).unwrap_or(0);
        let mut ztab = [0u8; 80];
        if sec == 0 || fat.read_at(d, &file, sec, &mut ztab)? != ztab.len() {
            return Ok(None);
        }
        let z = crate::storage::bytes::le32(&ztab, 14 * 4).unwrap_or(0);
        let zb = sec.checked_add(z).filter(|_| z != 0);
        let mut h = [0u8; 100];
        let Some(zb) = zb.filter(|&zb| fat.read_at(d, &file, zb, &mut h) == Ok(h.len())) else { return Ok(None) };
        let g = |i| crate::storage::bytes::le32(&h, i).unwrap_or(0);
        let range = [g(0x10), g(0x14), g(0x18), g(0x1c)];
        let (idx, len, data) = (h[0x20], h[0x21], g(0x22));
        let side = |a: u32, b: u32| u64::from(b.saturating_sub(a)) + 1;
        let tiles = side(range[0], range[1]) * side(range[2], range[3]);
        let ok = h[..6] == *b"BRYTON"
            && (1..=4).contains(&idx)
            && (1..=4).contains(&len)
            && range[0] <= range[1]
            && range[2] <= range[3]
            && range[1] < 1 << Z14_BITS
            && range[3] < 1 << Z14_BITS
            && u64::from(zb) + u64::from(data) <= u64::from(file.size)
            && u64::from(data) >= 100 + tiles * u64::from(idx);
        Ok(ok.then_some(Dat { file, zb, range, idx, len, data }))
    }

    /// Tile (`x`, `y`)'s place in the file: where its length starts (the bytes after it are the gzip stream) and
    /// how long the stream is; None when the file has no such tile.
    pub fn blob(&self, fat: &mut Fat, d: &mut impl Disk, x: u32, y: u32) -> Result<Option<(u32, u32)>, fat::Error> {
        let [x0, x1, y0, y1] = self.range;
        if x < x0 || x > x1 || y < y0 || y > y1 {
            return Ok(None);
        }
        let e = u64::from(y1 - y0 + 1) * u64::from(x - x0) + u64::from(y - y0);
        let mut b = [0u8; 4];
        let at = u64::from(self.zb) + 100 + e * u64::from(self.idx);
        let idx = usize::from(self.idx);
        let Ok(at) = u32::try_from(at) else { return Ok(None) };
        if fat.read_at(d, &self.file, at, b.get_mut(..idx).ok_or(fat::Error::Corrupt)?)? != idx {
            return Ok(None);
        }
        let off = u32::from_le_bytes(b);
        if off == u32::MAX >> (32 - 8 * idx) {
            return Ok(None); // all ones: no tile
        }
        let start = u64::from(self.zb) + u64::from(self.data) + u64::from(off);
        let Some(start) = u32::try_from(start).ok().filter(|&s| s < self.file.size) else { return Ok(None) };
        let mut l = [0u8; 4];
        let n = usize::from(self.len);
        let mut seek = Seek::new();
        if fat.read_from(d, &self.file, start, l.get_mut(..n).ok_or(fat::Error::Corrupt)?, &mut seek)? != n {
            return Ok(None);
        }
        let len = u32::from_le_bytes(l);
        let end = u64::from(start) + n as u64 + u64::from(len);
        Ok((len >= 18 && end <= u64::from(self.file.size)).then_some((start + n as u32, len)))
    }
}

/// z14 tile coordinates need this many bits.
const Z14_BITS: u32 = 14;

/// The z14 tile (x, y) holding a position (degrees x 1e-7), as stock numbers them (STORAGE_AND_MAPS.md 4.3).
///
/// ```
/// use bryton_rustfw::route::tile_of;
/// assert_eq!(tile_of(0, 0), (8192, 8192));
/// assert_eq!(tile_of(-520_000_000, -1_450_000_000), (1592, 10972));
/// ```
pub fn tile_of(lat: i32, lon: i32) -> (u32, u32) {
    use crate::tiles::{atanh, sin};
    let n = f64::from(1u32 << Z14_BITS);
    let phi = f64::from(lat.clamp(-850_000_000, 850_000_000)) / 1e7 * core::f64::consts::PI / 180.0;
    let x = (f64::from(lon) + 1.8e9) / 3.6e9 * n;
    let y = (1.0 - atanh(sin(phi)) / core::f64::consts::PI) / 2.0 * n;
    // not f64::clamp: its assertion message formats floats (see tiles.rs)
    let clamp = |t: f64| t.max(0.0).min(n - 1.0) as u32;
    (clamp(x), clamp(y))
}

// ---------------------------------------------------------------------------------------------------------------
// Files on the volume
// ---------------------------------------------------------------------------------------------------------------

/// What asking for a tile gave.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loaded {
    /// In the graph now.
    Yes,
    /// Not in any map file, or not usable (damaged, too big for the buffer): remembered as absent.
    No,
    /// The graph has no room: empty it ([`Graph::clear`]) and ask again.
    Full,
}

/// A map file looked for: for z9 tile `z9`, `dat` is the routing section when `usable`. Plain fields, no `Option`:
/// the all-zero value is what main.rs's static needs to stay in `.bss` (no 100 KB of initial data).
#[derive(Clone, Copy)]
struct Found {
    used: bool,
    usable: bool,
    z9: (u16, u16),
    dat: Dat,
}

impl Found {
    const ZERO: Found = Found { used: false, usable: false, z9: (0, 0), dat: Dat::ZERO };
}

/// Where a tile being loaded is: read and inflated, then decoded, then sorted ([`Maps::step`]).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Idle,
    Inflate,
    Decode,
    Sort,
}

/// A tile loaded a slice at a time: the stream's place in the file, the inflater and the readers of what it made.
struct Job {
    stage: Stage,
    tile: (u32, u32),
    file: File,
    /// The gzip stream: where it starts in the file and how long it is.
    blob: (u32, u32),
    seek: Seek,
    inf: Inflater,
    len: usize,
    dec: Dec,
    sort: Sort,
}

impl Job {
    const NONE: Job = Job {
        stage: Stage::Idle,
        tile: (0, 0),
        file: File { cluster: 0, size: 0, dir: false },
        blob: (0, 0),
        seek: Seek::new(),
        inf: Inflater::new(),
        len: 0,
        dec: Dec::ZERO,
        sort: Sort { phase: 0, i: 0, end: 0 },
    };
}

/// The map files in use: the last two z9 tiles' (a ride crosses a boundary now and then), found once.
pub struct Maps {
    found: [Found; 2],
    next: usize,
    job: Job,
    /// Tiles read and decoded, and bytes read from the card (for the cost, src/sim/).
    pub tiles: u32,
    /// See `tiles`.
    pub bytes: u32,
}

impl Default for Maps {
    fn default() -> Self {
        Self::new()
    }
}

impl Maps {
    /// No file looked for yet.
    pub const fn new() -> Maps {
        Maps { found: [Found::ZERO; 2], next: 0, job: Job::NONE, tiles: 0, bytes: 0 }
    }

    /// Looks for the files again (a card changed, or a retry).
    pub fn forget(&mut self) {
        self.found = [Found::ZERO; 2];
    }

    /// The file for z9 tile (`x`, `y`): `\Map\Update\<Region>`, then `\Map\MapData\<Region>`.
    fn locate(fat: &mut Fat, d: &mut impl Disk, x9: u16, y9: u16) -> Result<Option<File>, fat::Error> {
        let mut name = Line::<24>::new();
        name.push_str("9-");
        name.push_u32(u32::from(x9));
        name.push_str("-");
        name.push_u32(u32::from(y9));
        name.push_str(".dat");
        for base in ["Map/Update", "Map/MapData"] {
            let dir = match fat.open(d, base) {
                Ok(f) if f.dir => f,
                Err(fat::Error::Io) => return Err(fat::Error::Io),
                _ => continue,
            };
            let mut c = fat.cursor(dir)?;
            while let Some(e) = fat.list(d, &mut c)? {
                let n = e.name();
                let skip = [&b"Topo"[..], b"Contour", b"Route"].iter().any(|s| n.eq_ignore_ascii_case(s));
                if !e.file.dir || n.first() == Some(&b'.') || skip {
                    continue;
                }
                match fat.find(d, e.file, name.as_str()) {
                    Ok(f) if !f.dir && f.size > 0 => return Ok(Some(f)),
                    Err(fat::Error::Io) => return Err(fat::Error::Io),
                    _ => {}
                }
            }
        }
        Ok(None)
    }

    /// The tile being loaded, if a load is half done: [`Maps::step`] must be called for it (or [`Maps::abort`]).
    pub fn pending(&self) -> Option<(u32, u32)> {
        (self.job.stage != Stage::Idle).then_some(self.job.tile)
    }

    /// Gives up a load half done; the graph is as it was before the tile.
    pub fn abort(&mut self, g: &mut Graph) {
        if matches!(self.job.stage, Stage::Decode | Stage::Sort) {
            g.finish_tile(false);
        }
        self.job.stage = Stage::Idle;
    }

    /// Reads z14 tile (`x`, `y`) into `g` in one go (tests, tools): [`Maps::step`] with the whole tile as the slice.
    #[cfg(test)]
    pub fn load(
        &mut self,
        fat: &mut Fat,
        d: &mut impl Disk,
        g: &mut Graph,
        scratch: &mut Scratch<'_>,
        t: (u32, u32),
    ) -> Result<Loaded, fat::Error> {
        loop {
            if let Some(l) = self.step(fat, d, g, scratch, t, Slice::ALL)? {
                return Ok(l);
            }
        }
    }

    /// Reads z14 tile `t` into `g` a slice at a time. The first call looks for the file and the tile's place in it;
    /// then a call reads the stream a sector or two and inflates [`Slice::inflate`] bytes, then decodes
    /// [`Slice::nodes`] nodes a call, then sorts [`Slice::sort`] sifts a call: None while there is more (call again
    /// with the same `t`: [`Maps::pending`]), the answer at the end. An unusable tile is marked absent, so it is not
    /// asked for again. A card error is returned as it is, nothing remembered and the load dropped.
    #[inline(never)]
    pub fn step(
        &mut self,
        fat: &mut Fat,
        d: &mut impl Disk,
        g: &mut Graph,
        scratch: &mut Scratch<'_>,
        t: (u32, u32),
        slice: Slice,
    ) -> Result<Option<Loaded>, fat::Error> {
        let r = match self.job.stage {
            Stage::Idle => self.start(fat, d, g, t),
            Stage::Inflate => self.inflate(fat, d, g, scratch, slice),
            Stage::Decode => self.decode(g, scratch, slice),
            Stage::Sort => Ok(self.sort(g, slice)),
        };
        if r.is_err() {
            self.abort(g);
        }
        r
    }

    /// Finds the tile in its file; absent when there is none. Else the load is begun.
    fn start(
        &mut self,
        fat: &mut Fat,
        d: &mut impl Disk,
        g: &mut Graph,
        (x, y): (u32, u32),
    ) -> Result<Option<Loaded>, fat::Error> {
        let z9 = ((x >> Z9_SHIFT) as u16, (y >> Z9_SHIFT) as u16);
        let k = match self.found.iter().position(|f| f.used && f.z9 == z9) {
            Some(k) => k,
            None => {
                let dat = match Self::locate(fat, d, z9.0, z9.1)? {
                    Some(f) => Dat::open(fat, d, f)?,
                    None => None,
                };
                let k = self.next % self.found.len();
                self.next = k + 1;
                if let Some(slot) = self.found.get_mut(k) {
                    *slot = Found { used: true, usable: dat.is_some(), z9, dat: dat.unwrap_or(Dat::ZERO) };
                }
                k
            }
        };
        let Some(Found { usable: true, dat, .. }) = self.found.get(k).copied() else {
            return Ok(Some(self.absent(g, x, y)));
        };
        let Some(blob) = dat.blob(fat, d, x, y)? else { return Ok(Some(self.absent(g, x, y))) };
        let j = &mut self.job;
        (j.tile, j.file, j.blob, j.seek, j.inf, j.stage) =
            ((x, y), dat.file, blob, Seek::new(), Inflater::new(), Stage::Inflate);
        Ok(None)
    }

    /// The stream on from where the inflater is, a slice of output; at its end the tile's graph part is begun.
    fn inflate(
        &mut self,
        fat: &mut Fat,
        d: &mut impl Disk,
        g: &mut Graph,
        scratch: &mut Scratch<'_>,
        slice: Slice,
    ) -> Result<Option<Loaded>, fat::Error> {
        let (x, y) = self.job.tile;
        let (at, len) = self.job.blob;
        let used = self.job.inf.taken;
        // a sector at a time: the part of a read the slice does not use is read again by the next
        let mut io = [0u8; fat::SECTOR];
        let mut src = Bytes::new(
            fat,
            d,
            self.job.file,
            &mut self.job.seek,
            (at.saturating_add(used), len.saturating_sub(used)),
            &mut io,
        );
        let r = self.job.inf.step(&mut src, scratch, slice.inflate);
        if src.error == Some(fat::Error::Io) {
            return Err(fat::Error::Io);
        }
        self.bytes = self.bytes.saturating_add(self.job.inf.taken.saturating_sub(used));
        let n = match r {
            Ok(None) => return Ok(None),
            Ok(Some(n)) => n,
            Err(_) => {
                self.job.stage = Stage::Idle;
                return Ok(Some(self.absent(g, x, y)));
            }
        };
        match g.begin_tile(x, y) {
            Ok(()) => {
                (self.job.len, self.job.dec, self.job.sort, self.job.stage) =
                    (n, Dec::new(), Sort::default(), Stage::Decode);
                Ok(None)
            }
            Err(Error::Full) if g.is_empty() => Ok(Some(self.end_job_absent(g))),
            Err(Error::Full) => Ok(Some(self.end_job(Loaded::Full))),
            Err(_) => Ok(Some(self.end_job_absent(g))),
        }
    }

    fn decode(&mut self, g: &mut Graph, scratch: &Scratch<'_>, slice: Slice) -> Result<Option<Loaded>, fat::Error> {
        match g.decode_step(scratch, self.job.len, &mut self.job.dec, slice.nodes) {
            Ok(None) => Ok(None),
            Ok(Some(_)) => {
                self.job.stage = Stage::Sort;
                Ok(None)
            }
            Err(e) => {
                g.finish_tile(false);
                // a tile that does not fit an empty graph never will: absent, not asked for again
                Ok(Some(match e {
                    Error::Full if !g.is_empty() => self.end_job(Loaded::Full),
                    _ => self.end_job_absent(g),
                }))
            }
        }
    }

    fn sort(&mut self, g: &mut Graph, slice: Slice) -> Option<Loaded> {
        if !g.sort_tile(&mut self.job.sort, slice.sort) {
            return None;
        }
        g.finish_tile(true);
        self.tiles = self.tiles.saturating_add(1);
        Some(self.end_job(Loaded::Yes))
    }

    fn end_job(&mut self, l: Loaded) -> Loaded {
        self.job.stage = Stage::Idle;
        l
    }

    fn end_job_absent(&mut self, g: &mut Graph) -> Loaded {
        let (x, y) = self.job.tile;
        self.job.stage = Stage::Idle;
        self.absent(g, x, y)
    }

    fn absent(&self, g: &mut Graph, x: u32, y: u32) -> Loaded {
        if g.mark_absent(x, y) { Loaded::No } else { Loaded::Full }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::*;
    use crate::fat::tests::{Ram, image};
    use std::vec::Vec;

    /// The synthetic file's name: its z9 tile (tools/route_pack.py `fixture`).
    pub const NAME: &str = "9-49-342.dat";
    /// Where the made-up network starts (open South Pacific): degrees x 1e-7.
    pub const ORIGIN: (i32, i32) = (-520_000_000, -1_450_000_000);
    /// The z14 tiles with data, and the empty one inside the file's range.
    pub const FIXTURE_TILES: [(u32, u32); 8] = [
        (1592, 10972),
        (1593, 10971),
        (1593, 10972),
        (1594, 10972),
        (1595, 10971),
        (1595, 10972),
        (1596, 10970),
        (1596, 10971),
    ];
    pub const EMPTY: (u32, u32) = (1593, 10973);

    /// A place `x` metres east and `y` north of [`ORIGIN`], degrees x 1e-7 (tools/route_pack.py `metres`).
    pub fn metres(x: f64, y: f64) -> (i32, i32) {
        let cos = 40_295.0 / 65_536.0; // map::cos_q16 at the origin's latitude
        (ORIGIN.0 + (y * 1e6 / 11_132.0).round() as i32, ORIGIN.1 + (x * 1e6 / (11_132.0 * cos)).round() as i32)
    }

    /// The synthetic map file made by tools/route_pack.py.
    pub fn fixture() -> Vec<u8> {
        std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/test-data/route/9-49-342.dat")).unwrap()
    }

    /// The test volume with the synthetic map file in `\Map\MapData\Test`, as the owner copies stock's.
    pub fn image_with_map() -> Vec<u8> {
        image_with_file(NAME, &fixture())
    }

    /// The test volume with a map file `bytes` called `name` in `\Map\MapData\Test`.
    pub fn image_with_file(name: &str, bytes: &[u8]) -> Vec<u8> {
        let mut d = Ram(image(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let t = fat::write::dos_time((2026, 10, 26, 12, 0, 0));
        let map = fat.ensure_dir(&mut d, fat.root(), "Map", t).unwrap();
        let data = fat.ensure_dir(&mut d, map, "MapData", t).unwrap();
        let region = fat.ensure_dir(&mut d, data, "Test", t).unwrap();
        let len = bytes.len() as u32;
        let mut padded = bytes.to_vec();
        padded.resize(padded.len().div_ceil(fat::SECTOR) * fat::SECTOR, 0);
        let mut h = fat.create(&mut d, region, name, t).unwrap();
        fat.write_sectors(&mut d, &mut h, 0, &padded).unwrap();
        fat.set_size(&mut d, &mut h, len, t).unwrap();
        d.0
    }

    pub fn scratch() -> Vec<u16> {
        std::vec![0u16; 48 * 1024]
    }

    /// Every tile of the fixture loaded, one after the other.
    fn load_all(g: &mut Graph, maps: &mut Maps, d: &mut Ram, fat: &mut Fat) {
        let mut mem = scratch();
        for t in FIXTURE_TILES {
            assert_eq!(maps.load(fat, d, g, &mut Scratch(&mut mem), t), Ok(Loaded::Yes), "{t:?}");
        }
    }

    #[test]
    fn tile_numbers() {
        assert_eq!(tile_of(ORIGIN.0, ORIGIN.1), (1592, 10972));
        assert_eq!(tile_of(0, 0), (8192, 8192));
        assert_eq!(tile_of(i32::MAX, i32::MAX), (16383, 26), "off the map: clamped (at 85 degrees)");
        // a tile's own centre, which a routing tile takes as its base (tools/route_pack.py `centre`)
        assert_eq!(tile_of(-520_051_740, -1_450_085_449), (1592, 10972));
        assert_eq!(tile_of(-519_916_458, -1_449_426_270), (1595, 10971));
    }

    /// The made-up file read whole: nodes, elevations, ways, the junction, the dead end; every edge's far end is
    /// found, in its own tile or the next, and listed at both ends.
    #[test]
    fn reads_the_fixture() {
        let mut d = Ram(image_with_map(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let (mut g, mut maps) = (Graph::new(), Maps::new());
        load_all(&mut g, &mut maps, &mut d, &mut fat);
        assert_eq!((g.len(), maps.tiles), (325, 8));
        assert!(maps.bytes > 3000 && maps.bytes < 5000, "{} bytes read", maps.bytes);
        for t in FIXTURE_TILES {
            assert_eq!(g.state(t.0, t.1), TileState::Loaded);
        }
        let mut edges = 0;
        for (i, n) in g.nodes().iter().enumerate() {
            assert_eq!(g.find(n.lat, n.lon), Some(i), "sorted: found at its own place");
            for ((lat, lon), _) in g.edges(i) {
                let j = g.find(lat, lon).expect("the far end is a node");
                assert!(g.edges(j).any(|(p, _)| p == (n.lat, n.lon)), "listed at both ends");
                edges += 1;
            }
        }
        assert_eq!(edges, 2 * 324, "a tree of 325 nodes: 324 roads");
        let at = |x, y| {
            let (lat, lon) = metres(x, y);
            g.find(lat, lon).unwrap_or_else(|| panic!("no node at {x},{y}"))
        };
        let elev = |x, y| g.node(at(x, y)).unwrap().elev;
        assert_eq!([0.0, 975.0, 1000.0, 2000.0, 3000.0, 3200.0].map(|x| elev(x, 0.0)), [100, 100, 100, 160, 220, 220]);
        // the junction at 3.2 km: three roads, two ways; the branch's end: one road
        assert_eq!(g.edges(at(3200.0, 0.0)).count(), 3);
        assert_eq!(g.node(at(3200.0, 0.0)).unwrap().ways.iter().filter(|&&w| w != 0).count(), 2);
        assert_eq!(g.edges(at(4000.0, 0.0)).count(), 1, "dead end");
        // the side road's other class
        let side = g.edges(at(1500.0, 0.0)).map(|(_, a)| a).collect::<Vec<_>>();
        assert!(side.contains(&0x99) && side.contains(&0xee), "{side:?}");
        // a way shared by two neighbours; not by the branch and the road it leaves
        let (a, b) = (g.node(at(3175.0, 0.0)).unwrap(), g.node(at(3200.0, 0.0)).unwrap());
        assert!(a.common_way(b).is_some());
        let c = g.node(at(3225.0, 0.0)).unwrap();
        let m = g.node(at(3200.0 + 25.0 * 40f64.to_radians().cos(), 25.0 * 40f64.to_radians().sin())).unwrap();
        assert!(c.common_way(m).is_none(), "the branch and the bend are different ways");
    }

    /// A xorshift step (seeded: failures reproduce).
    fn next(x: &mut u32) -> u32 {
        *x ^= *x << 13;
        *x ^= *x >> 17;
        *x ^= *x << 5;
        *x
    }

    /// Tile `t` loaded in slices of random sizes (1 to `most` of each kind); the answer and the calls it took.
    pub fn load_sliced(
        maps: &mut Maps,
        fat: &mut Fat,
        d: &mut Ram,
        g: &mut Graph,
        mem: &mut [u16],
        t: (u32, u32),
        (seed, most): (&mut u32, u32),
    ) -> (Result<Loaded, fat::Error>, u32) {
        for calls in 1.. {
            let mut r = |m: u32| (next(seed) % m + 1) as usize;
            let slice = Slice { inflate: r(most * 40), nodes: r(most), sort: r(most * 3) };
            match maps.step(fat, d, g, &mut Scratch(mem), t, slice) {
                Ok(None) => assert_eq!(maps.pending(), Some(t)),
                Ok(Some(l)) => return (Ok(l), calls),
                Err(e) => return (Err(e), calls),
            }
        }
        unreachable!()
    }

    /// The same nodes, edges and tile states.
    pub fn same(a: &Graph, b: &Graph) {
        assert_eq!(a.nodes(), b.nodes());
        assert_eq!(a.to[..a.n_edges], b.to[..b.n_edges]);
        assert_eq!(a.attr[..a.n_edges], b.attr[..b.n_edges]);
        assert_eq!(a.n_slots, b.n_slots);
        for (x, y) in a.slots[..a.n_slots].iter().zip(&b.slots[..b.n_slots]) {
            assert_eq!((x.x, x.y, x.start, x.end, x.loaded, x.bbox), (y.x, y.y, y.start, y.end, y.loaded, y.bbox));
        }
    }

    /// A tile loaded in many slices of random sizes is the tile loaded in one go, whatever the cut points: the
    /// same graph and the same bytes counted, with more than one call, the inflate, the decode and the sort each
    /// cut.
    #[test]
    fn slices_make_the_same_graph() {
        let mut seed = 0x1234_5678u32;
        for most in [1, 3, 20, 200] {
            let mut d = Ram(image_with_map(), 0);
            let mut fat = Fat::mount(&mut d).unwrap();
            let (mut g1, mut m1, mut mem) = (Graph::new(), Maps::new(), scratch());
            let (mut g2, mut m2) = (Graph::new(), Maps::new());
            let mut calls = 0;
            for t in FIXTURE_TILES.into_iter().chain([EMPTY]) {
                let one = m1.load(&mut fat, &mut d, &mut g1, &mut Scratch(&mut mem), t);
                let (many, n) = load_sliced(&mut m2, &mut fat, &mut d, &mut g2, &mut mem, t, (&mut seed, most));
                assert_eq!(one, many, "{t:?}");
                assert_eq!(m2.pending(), None);
                same(&g1, &g2);
                calls += n;
            }
            assert_eq!((m1.tiles, m1.bytes), (m2.tiles, m2.bytes));
            assert!(calls > 9 * 3 || most > 20, "{calls} calls for 9 tiles with slices up to {most}");
        }
    }

    /// A damaged file gives the same answer sliced or whole, and a load given up half done ([`Maps::abort`], at
    /// every call of a tile) leaves the graph as it was.
    #[test]
    fn slices_agree_on_damage_and_abort() {
        let img = image_with_map();
        let mut d = Ram(img.clone(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let f = fat.open(&mut d, "Map/MapData/Test/9-49-342.dat").unwrap();
        let (base, size) = (fat.lba(f.cluster) as usize * fat::SECTOR, f.size as usize);
        let mut seed = 99u32;
        let mut mem = scratch();
        for k in 0..400 {
            let mut d = Ram(img.clone(), 0);
            let at = (next(&mut seed) as usize) % size;
            d.0[base + at] ^= 1 << (k % 8);
            let mut fat = Fat::mount(&mut d).unwrap();
            let (mut g1, mut m1, mut g2, mut m2) = (Graph::new(), Maps::new(), Graph::new(), Maps::new());
            for t in FIXTURE_TILES {
                let one = m1.load(&mut fat, &mut d, &mut g1, &mut Scratch(&mut mem), t);
                let (many, _) = load_sliced(&mut m2, &mut fat, &mut d, &mut g2, &mut mem, t, (&mut seed, 5));
                assert_eq!(one, many, "flip at {at}");
                same(&g1, &g2);
            }
        }
        // abort after each number of calls: nothing of the tile stays in the graph
        let mut d = Ram(img, 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let t = FIXTURE_TILES[2];
        for stop in 0..30 {
            let (mut g, mut maps) = (Graph::new(), Maps::new());
            let slice = Slice { inflate: 100, nodes: 4, sort: 10 };
            for _ in 0..stop {
                if maps.step(&mut fat, &mut d, &mut g, &mut Scratch(&mut mem), t, slice).unwrap().is_some() {
                    break;
                }
            }
            maps.abort(&mut g);
            assert!(maps.pending().is_none() && g.is_empty() && g.state(t.0, t.1) == TileState::Unknown, "{stop}");
            // and the tile loads whole afterwards
            assert_eq!(maps.load(&mut fat, &mut d, &mut g, &mut Scratch(&mut mem), t), Ok(Loaded::Yes));
        }
    }

    /// One tile alone: the edges that leave it lead to nodes not found (the next tile is not loaded yet).
    #[test]
    fn a_neighbour_in_a_tile_not_loaded() {
        let mut d = Ram(image_with_map(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let (mut g, mut maps, mut mem) = (Graph::new(), Maps::new(), scratch());
        assert_eq!(maps.load(&mut fat, &mut d, &mut g, &mut Scratch(&mut mem), FIXTURE_TILES[2]), Ok(Loaded::Yes));
        let mut lost = 0;
        for i in 0..g.len() {
            lost += g.edges(i).filter(|&((lat, lon), _)| g.find(lat, lon).is_none()).count();
        }
        assert!(lost > 0 && lost <= 4, "{lost} edges leave the tile");
        assert_eq!(g.state(FIXTURE_TILES[1].0, FIXTURE_TILES[1].1), TileState::Unknown);
    }

    /// No file, no tile in the file, outside its range: absent, remembered; the files are looked for once.
    #[test]
    fn absent_tiles() {
        let mut mem = scratch();
        let mut s = Scratch(&mut mem);
        let mut d = Ram(image_with_map(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let (mut g, mut maps) = (Graph::new(), Maps::new());
        assert_eq!(maps.load(&mut fat, &mut d, &mut g, &mut s, EMPTY), Ok(Loaded::No), "an empty cell in the range");
        assert_eq!(maps.load(&mut fat, &mut d, &mut g, &mut s, (1600, 10972)), Ok(Loaded::No), "past the range");
        assert_eq!(maps.load(&mut fat, &mut d, &mut g, &mut s, (5, 5)), Ok(Loaded::No), "another z9 tile: no file");
        for t in [EMPTY, (1600, 10972), (5, 5)] {
            assert_eq!(g.state(t.0, t.1), TileState::Absent);
        }
        let reads = d.1;
        assert_eq!(maps.load(&mut fat, &mut d, &mut g, &mut s, (1600, 10973)), Ok(Loaded::No));
        assert!(d.1 - reads < 20, "the file is not looked for again: {} reads", d.1 - reads);
        // a volume without any map
        let mut d = Ram(image(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let mut maps = Maps::new();
        assert_eq!(maps.load(&mut fat, &mut d, &mut g, &mut s, FIXTURE_TILES[0]), Ok(Loaded::No));
        // a scratch buffer too small for the tile: absent too
        let mut d = Ram(image_with_map(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let (mut g, mut maps, mut small) = (Graph::new(), Maps::new(), std::vec![0u16; 50]);
        assert_eq!(maps.load(&mut fat, &mut d, &mut g, &mut Scratch(&mut small), FIXTURE_TILES[2]), Ok(Loaded::No));
        assert_eq!(g.state(FIXTURE_TILES[2].0, FIXTURE_TILES[2].1), TileState::Absent);
    }

    /// The graph's capacity: a tile that does not fit is refused whole, and the graph is as it was; the tile table
    /// too.
    #[test]
    fn graph_capacity() {
        let mut g = Graph::new();
        g.begin_tile(1, 1).unwrap();
        for i in 0..10 {
            g.add_node((i, 0, 5), [1, 0], &[(i + 1, 0, 7), (i - 1, 0, 7)]).unwrap();
        }
        g.end_tile(true);
        assert_eq!((g.len(), g.edges(3).count(), g.state(1, 1)), (10, 2, TileState::Loaded));
        g.begin_tile(2, 1).unwrap();
        let mut full = None;
        for i in 0..NODES as i32 {
            if let Err(e) = g.add_node((1000 + i, 0, 0), [0; 2], &[(0, 0, 0); 3]) {
                full = Some((i, e));
                break;
            }
        }
        assert_eq!(full.map(|f| f.1), Some(Error::Full));
        g.end_tile(false);
        assert_eq!((g.len(), g.edges(3).count(), g.state(2, 1)), (10, 2, TileState::Unknown), "as it was");
        // edges run out before the nodes: 3 each
        g.begin_tile(2, 1).unwrap();
        let n =
            (0..NODES as i32).take_while(|&i| g.add_node((1000 + i, 0, 0), [0; 2], &[(0, 0, 0); 3]).is_ok()).count();
        assert_eq!(n, (EDGES - 20) / 3);
        g.end_tile(false);
        assert_eq!(g.len(), 10);
        // the tile table
        for k in 0..super::TILES as u32 - 1 {
            assert!(g.mark_absent(10 + k, 0));
        }
        assert!(!g.mark_absent(99, 0), "full");
        assert_eq!(g.begin_tile(3, 3), Err(Error::Full));
        g.clear();
        assert_eq!((g.len(), g.state(1, 1)), (0, TileState::Unknown));
    }

    /// A tile with more nodes than the graph holds, asked for on an empty graph: absent, and not read again (it was
    /// asked for every 30 s).
    #[test]
    fn a_tile_too_big_for_the_graph_is_absent() {
        fn varint(mut v: u64, out: &mut Vec<u8>) {
            loop {
                out.push((v & 0x7f) as u8 | if v >> 7 != 0 { 0x80 } else { 0 });
                v >>= 7;
                if v == 0 {
                    return;
                }
            }
        }
        let mut msg = std::vec![0x08, 0, 0x10, 0, 0x18, 20, 0x20, 2];
        for i in 0..NODES as u64 + 50 {
            let mut node = std::vec![0x08];
            varint(i << 20, &mut node);
            msg.push(0x2a);
            msg.push(node.len() as u8);
            msg.extend(node);
        }
        // gzip, stored blocks
        let mut gz = std::vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 3];
        let chunks: Vec<&[u8]> = msg.chunks(60_000).collect();
        for (k, c) in chunks.iter().enumerate() {
            gz.push(u8::from(k + 1 == chunks.len()));
            gz.extend((c.len() as u16).to_le_bytes());
            gz.extend((!(c.len() as u16)).to_le_bytes());
            gz.extend_from_slice(c);
        }
        gz.extend(crate::inflate::crc32(msg.iter().copied()).to_le_bytes());
        gz.extend((msg.len() as u32).to_le_bytes());
        // the container: header, zoom table, zoom header, one index entry, the tile
        let (x, y) = FIXTURE_TILES[0];
        let mut file = std::vec![0u8; 0x12];
        file[0x0e..0x12].copy_from_slice(&0x12u32.to_le_bytes());
        let mut ztab = [0u32; 20];
        ztab[14] = 80;
        file.extend(ztab.iter().flat_map(|z| z.to_le_bytes()));
        let mut zh = [0u8; 100];
        zh[..6].copy_from_slice(b"BRYTON");
        for (k, v) in [x, x, y, y].into_iter().enumerate() {
            zh[0x10 + 4 * k..0x14 + 4 * k].copy_from_slice(&v.to_le_bytes());
        }
        (zh[0x20], zh[0x21]) = (4, 4);
        zh[0x22..0x26].copy_from_slice(&104u32.to_le_bytes());
        file.extend(zh);
        file.extend(0u32.to_le_bytes());
        file.extend((gz.len() as u32).to_le_bytes());
        file.extend(gz);
        let mut d = Ram(image_with_file(NAME, &file), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let (mut g, mut maps, mut mem) = (Graph::new(), Maps::new(), std::vec![0u16; SCRATCH_WORDS]);
        let mut s = Scratch(&mut mem);
        assert_eq!(maps.load(&mut fat, &mut d, &mut g, &mut s, (x, y)), Ok(Loaded::No));
        assert_eq!((g.state(x, y), g.len()), (TileState::Absent, 0));
        assert!(maps.bytes > 20_000, "it was read once");
    }

    /// Tile messages that are not: a wrong transport count, deltaBits out of range, a field cut short. Refused whole.
    #[test]
    fn refuses_bad_messages() {
        let put = |bytes: &[u8]| {
            let mut mem = std::vec![0u16; 2000];
            let mut s = Scratch(&mut mem);
            for (i, &b) in bytes.iter().enumerate() {
                assert!(s.set(i, b));
            }
            let mut g = Graph::new();
            let r = g.decode(1, 1, &s, bytes.len());
            (r, g.len(), g.state(1, 1))
        };
        // base 0 0, deltaBits 20, transport 2, one node: key 0 (the base) and elevation 2000 / 5 - 500 = -100
        let good = [0x08, 0, 0x10, 0, 0x18, 20, 0x20, 2, 0x2a, 5, 0x08, 0, 0x10, 0xd0, 0x0f];
        assert_eq!(put(&good), (Ok(1), 1, TileState::Loaded));
        assert_eq!(put(&[0x08, 0, 0x10, 0, 0x18, 20, 0x20, 3]).0, Err(Error::Bad), "transport");
        assert_eq!(put(&[0x08, 0, 0x10, 0, 0x18, 1, 0x20, 2]).0, Err(Error::Bad), "bits");
        assert_eq!(put(&[0x08, 0, 0x10, 0, 0x18, 32, 0x20, 2]).0, Err(Error::Bad), "bits");
        assert_eq!(put(&[0x08]).0, Err(Error::Bad), "no value");
        assert_eq!(
            put(&[0x08, 0, 0x10, 0, 0x18, 20, 0x20, 2, 0x2a, 9, 0x08]).0,
            Err(Error::Bad),
            "length past the end"
        );
        assert_eq!(put(&[0x07]).0, Err(Error::Bad), "wire type 7");
        let (r, n, state) = put(&[0x08, 0, 0x10, 0, 0x18, 20, 0x20, 2, 0x2a, 3, 0x08, 0x80, 0x80]);
        assert_eq!((r, n, state), (Err(Error::Bad), 0, TileState::Unknown), "a varint that never ends");
        assert_eq!(put(&[]).0, Err(Error::Bad), "empty");
    }

    /// The container damaged anywhere in its first bytes and its index: a refusal or a tile, never a panic, and a
    /// tile that loads is still the made-up road (the gzip CRC checks the rest).
    #[test]
    fn damaged_container() {
        let img = image_with_map();
        let mut d = Ram(img.clone(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let f = fat.open(&mut d, "Map/MapData/Test/9-49-342.dat").unwrap();
        let base = fat.lba(f.cluster) as usize * fat::SECTOR;
        assert_eq!(&img[base..base + f.size as usize], &fixture()[..], "the file is contiguous");
        let mut mem = scratch();
        let mut loaded = 0;
        for at in (0..0x12 + 80 + 100 + 4 * 20).chain(f.size as usize - 40..f.size as usize) {
            for flip in [0x01u8, 0x80, 0xff] {
                let mut d = Ram(img.clone(), 0);
                d.0[base + at] ^= flip;
                let mut fat = Fat::mount(&mut d).unwrap();
                let (mut g, mut maps) = (Graph::new(), Maps::new());
                for t in FIXTURE_TILES {
                    if maps.load(&mut fat, &mut d, &mut g, &mut Scratch(&mut mem), t) == Ok(Loaded::Yes) {
                        loaded += 1;
                    }
                }
                assert!(g.len() <= 325);
            }
        }
        assert!(loaded > 100, "most flips leave most tiles readable: {loaded}");
    }

    /// Every tile of a real file (a z9 `.dat` from stock's maps) loads: the nodes, the time on the computer.
    /// Ignored: real maps stay outside the repository (AGENTS.md, "Public repository"); point BRYTON_ROUTE_DAT at
    /// the file (`cargo test-host real_file -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn real_file() {
        let Ok(path) = std::env::var("BRYTON_ROUTE_DAT") else {
            std::println!("BRYTON_ROUTE_DAT not set: skipped");
            return;
        };
        let bytes = std::fs::read(&path).unwrap();
        let name = std::string::String::from(std::path::Path::new(&path).file_name().unwrap().to_str().unwrap());
        let mut d = Ram(image_with_file(&name, &bytes), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let z: Vec<u32> = name.trim_end_matches(".dat").split('-').skip(1).map(|s| s.parse::<u32>().unwrap()).collect();
        let mut mem = std::vec![0u16; SCRATCH_WORDS]; // the unit's own size
        let (mut g, mut maps) = (Graph::new(), Maps::new());
        let (mut ok, mut absent, mut full, mut nodes) = (0, 0, 0, 0);
        let mut largest = (0, 0, 0, 0, 0);
        let (mut g2, mut m2, mut seed) = (Graph::new(), Maps::new(), 7u32);
        let t0 = std::time::Instant::now();
        for x in z[0] << 5..(z[0] + 1) << 5 {
            for y in z[1] << 5..(z[1] + 1) << 5 {
                g.clear();
                let b0 = maps.bytes;
                match maps.load(&mut fat, &mut d, &mut g, &mut Scratch(&mut mem), (x, y)).unwrap() {
                    Loaded::Yes => {
                        (ok, nodes) = (ok + 1, nodes + g.len());
                        let edges: usize = (0..g.len()).map(|i| g.edges(i).count()).sum();
                        largest = largest.max((g.len(), edges, maps.bytes - b0, x, y));
                        // the same tile in slices cut at random places: the same graph
                        g2.clear();
                        let (r, _) =
                            load_sliced(&mut m2, &mut fat, &mut d, &mut g2, &mut mem, (x, y), (&mut seed, 200));
                        assert_eq!(r, Ok(Loaded::Yes));
                        same(&g, &g2);
                    }
                    Loaded::No => absent += 1,
                    Loaded::Full => full += 1,
                }
            }
        }
        std::println!(
            "{ok} tiles loaded ({nodes} nodes), {absent} absent or too big, {full} full; {} us a tile",
            t0.elapsed().as_micros() / 1024
        );
        std::println!("largest tile loaded: {} nodes, {} edges, {} compressed bytes", largest.0, largest.1, largest.2);
        assert!(ok > 900);

        // The largest tile in the slices of a main loop pass: what each call does, and the time on the unit
        // estimated from the instructions each kind of work takes (a bit decoded 25, a byte out 30 with its CRC, a
        // node 900, a sift 400) scaled so that the whole tile costs what the unit measured for its slowest one
        // (237.5 ms for a tile load in one go, 2026-10-26, tools/dev.py info).
        let t = (largest.3, largest.4);
        g.clear();
        let mut calls = std::vec::Vec::new(); // (instructions, kind)
        loop {
            let (stage, n0, in0, nodes0) =
                (maps.job.stage, maps.job.inf.written(), maps.job.inf.taken, maps.job.dec.nodes);
            let r = maps.step(&mut fat, &mut d, &mut g, &mut Scratch(&mut mem), t, Slice::PASS).unwrap();
            let j = &maps.job;
            let instr = match stage {
                Stage::Inflate => (j.inf.written() - n0) * 30 + (j.inf.taken - in0) as usize * 200,
                Stage::Decode => (j.dec.nodes - nodes0) * 900,
                Stage::Sort => Slice::PASS.sort * 400,
                Stage::Idle => 2000,
            };
            calls.push((instr, stage as u8));
            if r.is_some() {
                break;
            }
        }
        same(&g, &{
            let mut one = Graph::new();
            Maps::new().load(&mut fat, &mut d, &mut one, &mut Scratch(&mut mem), t).unwrap();
            one
        });
        let total: usize = calls.iter().map(|c| c.0).sum();
        let us_per_instr = 237_500.0 / total as f64;
        let worst = calls.iter().map(|c| c.0).max().unwrap();
        std::println!(
            "largest tile {t:?}: {} nodes, {} calls, {total} instructions, {:.2} cycles an instruction at the measured 237.5 ms; \
             a call at most {worst} instructions = {:.1} ms, {} calls over 5 ms",
            largest.0,
            calls.len(),
            us_per_instr * 200.0,
            worst as f64 * us_per_instr / 1000.0,
            calls.iter().filter(|c| c.0 as f64 * us_per_instr > 5000.0).count()
        );
    }
}
