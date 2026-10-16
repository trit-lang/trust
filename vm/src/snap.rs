//! Machine snapshots: the full state of a `Vm`, on disk.
//!
//! Execution is deterministic given an image and its input, so a snapshot
//! near a fault plus a store watchpoint finds in seconds of replay what a
//! rerun from reset finds only after hours. Built for the DDC hunt
//! (`target/HANDOFF.md`): the format is deliberately boring — little-endian,
//! no compression, one pass in each direction.

use crate::vm::Io;

/// The whole machine: registers, pc, input position, and every page of
/// memory that was ever touched.
pub struct Snap {
    /// The address-space size the machine was built with (`--mem`).
    pub mem_size: i128,
    /// Where the next instruction comes from.
    pub pc: i128,
    /// Instructions retired so far.
    pub steps: u64,
    /// The register file, laid out −13 … +13.
    pub regs: [i128; 27],
    /// The character ports: unconsumed input and output so far.
    pub io: Io,
    /// Every allocated page, ascending by address.
    pub pages: Vec<(i128, Box<[i16]>)>,
}

const MAGIC: &[u8; 7] = b"TSNAP\x00\x01";

fn bad(why: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, why.to_string())
}

impl Snap {
    /// Serialize. `output` is deliberately not persisted: the machine cannot
    /// read its own output, so a replay is identical without it.
    pub fn write_to(&self, w: &mut impl std::io::Write) -> std::io::Result<()> {
        w.write_all(MAGIC)?;
        w.write_all(&(self.mem_size as i64).to_le_bytes())?;
        w.write_all(&(self.pc as i64).to_le_bytes())?;
        w.write_all(&self.steps.to_le_bytes())?;
        for &r in &self.regs {
            w.write_all(&(r as i64).to_le_bytes())?;
        }
        w.write_all(&[u8::from(self.io.is_closed())])?;
        let pending = self.io.pending_bytes();
        w.write_all(&(pending.len() as u64).to_le_bytes())?;
        w.write_all(&pending)?;
        w.write_all(&(self.pages.len() as u64).to_le_bytes())?;
        for (start, contents) in &self.pages {
            w.write_all(&(*start as i64).to_le_bytes())?;
            w.write_all(&(contents.len() as u64).to_le_bytes())?;
            for &t in contents.iter() {
                w.write_all(&t.to_le_bytes())?;
            }
        }
        Ok(())
    }

    /// Parse what `write_to` wrote.
    pub fn read_from(r: &mut impl std::io::Read) -> std::io::Result<Snap> {
        let mut magic = [0u8; 7];
        r.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(bad("not a tritium snapshot"));
        }
        let mem_size = read_i64(r)? as i128;
        let pc = read_i64(r)? as i128;
        let steps = read_u64(r)?;
        let mut regs = [0i128; 27];
        for slot in &mut regs {
            *slot = read_i64(r)? as i128;
        }
        let mut closed = [0u8; 1];
        r.read_exact(&mut closed)?;
        let pending_len = read_u64(r)? as usize;
        let mut pending = vec![0u8; pending_len];
        r.read_exact(&mut pending)?;
        let io = Io::from_pending(pending, closed[0] != 0);

        let page_count = read_u64(r)?;
        let mut pages = Vec::with_capacity(page_count as usize);
        for _ in 0..page_count {
            let start = read_i64(r)? as i128;
            let len = read_u64(r)? as usize;
            let mut contents = Vec::with_capacity(len);
            for _ in 0..len {
                let mut b = [0u8; 2];
                r.read_exact(&mut b)?;
                contents.push(i16::from_le_bytes(b));
            }
            pages.push((start, contents.into_boxed_slice()));
        }
        Ok(Snap {
            mem_size,
            pc,
            steps,
            regs,
            io,
            pages,
        })
    }

    /// Write to a file, atomically: a crash mid-write leaves the previous
    /// snapshot intact rather than half of a new one.
    pub fn write_file(&self, path: &str) -> std::io::Result<()> {
        let tmp = format!("{path}.tmp");
        {
            let f = std::fs::File::create(&tmp)?;
            let mut w = std::io::BufWriter::new(f);
            self.write_to(&mut w)?;
            std::io::Write::flush(&mut w)?;
        }
        std::fs::rename(&tmp, path)
    }

    /// Read a snapshot file.
    pub fn read_file(path: &str) -> std::io::Result<Snap> {
        let f = std::fs::File::open(path)?;
        Snap::read_from(&mut std::io::BufReader::new(f))
    }
}

fn read_u64(r: &mut impl std::io::Read) -> std::io::Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

fn read_i64(r: &mut impl std::io::Read) -> std::io::Result<i64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(i64::from_le_bytes(b))
}
