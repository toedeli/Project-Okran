//! The wire format: each message is a u32 length, a tag byte and its fields, little-endian.

use std::io::{self, Read};

pub const VERSION: u32 = 1;

/// A player's order, run on every peer at the same tick.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    MoveSquad { ids: Vec<u16>, to: [f32; 3] },
    Attack { ids: Vec<u16>, whom: u16 },
    Stop { ids: Vec<u16> },
}

/// One person as a server snapshot carries them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PersonState {
    pub id: u16,
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    /// 1 down, 2 dead.
    pub flags: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    /// Sent first, both ways: the protocol and the world as loaded (its state hash).
    Hello { version: u32, world: u64 },
    /// One player's orders for a tick, often none.
    Orders { tick: u64, commands: Vec<Command> },
    /// The sender's state hash after a tick.
    Hash { tick: u64, hash: u64 },
    /// Server snapshots: everyone's state after a tick.
    Snapshot { tick: u64, people: Vec<PersonState> },
    /// The sender has run all its ticks.
    Done,
}

/// The message with its length in front, ready to write.
pub fn encode(msg: &Msg) -> Vec<u8> {
    let mut b = vec![0u8; 4];
    match msg {
        Msg::Hello { version, world } => {
            b.push(1);
            b.extend(version.to_le_bytes());
            b.extend(world.to_le_bytes());
        }
        Msg::Orders { tick, commands } => {
            b.push(2);
            b.extend(tick.to_le_bytes());
            b.push(commands.len() as u8);
            for c in commands {
                encode_command(&mut b, c);
            }
        }
        Msg::Hash { tick, hash } => {
            b.push(3);
            b.extend(tick.to_le_bytes());
            b.extend(hash.to_le_bytes());
        }
        Msg::Snapshot { tick, people } => {
            b.push(4);
            b.extend(tick.to_le_bytes());
            b.extend((people.len() as u16).to_le_bytes());
            for p in people {
                b.extend(p.id.to_le_bytes());
                for v in p.position.iter().chain(&p.velocity) {
                    b.extend(v.to_le_bytes());
                }
                b.push(p.flags);
            }
        }
        Msg::Done => b.push(5),
    }
    let len = (b.len() - 4) as u32;
    b[..4].copy_from_slice(&len.to_le_bytes());
    b
}

fn encode_command(b: &mut Vec<u8>, c: &Command) {
    let ids = |b: &mut Vec<u8>, ids: &[u16]| {
        b.push(ids.len() as u8);
        for i in ids {
            b.extend(i.to_le_bytes());
        }
    };
    match c {
        Command::MoveSquad { ids: v, to } => {
            b.push(1);
            ids(b, v);
            for x in to {
                b.extend(x.to_le_bytes());
            }
        }
        Command::Attack { ids: v, whom } => {
            b.push(2);
            ids(b, v);
            b.extend(whom.to_le_bytes());
        }
        Command::Stop { ids: v } => {
            b.push(3);
            ids(b, v);
        }
    }
}

/// Reads one message.
pub fn read_msg(r: &mut impl Read) -> io::Result<Msg> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > 16 << 20 {
        return Err(bad("message too long"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    decode(&body)
}

/// A message from its bytes after the length.
pub fn decode(body: &[u8]) -> io::Result<Msg> {
    let mut c = Cursor { b: body, i: 0 };
    let msg = match c.u8()? {
        1 => Msg::Hello { version: c.u32()?, world: c.u64()? },
        2 => {
            let tick = c.u64()?;
            let n = c.u8()?;
            let mut commands = Vec::with_capacity(n as usize);
            for _ in 0..n {
                commands.push(decode_command(&mut c)?);
            }
            Msg::Orders { tick, commands }
        }
        3 => Msg::Hash { tick: c.u64()?, hash: c.u64()? },
        4 => {
            let tick = c.u64()?;
            let n = c.u16()?;
            let mut people = Vec::with_capacity(n as usize);
            for _ in 0..n {
                let id = c.u16()?;
                let position = [c.f32()?, c.f32()?, c.f32()?];
                let velocity = [c.f32()?, c.f32()?, c.f32()?];
                people.push(PersonState { id, position, velocity, flags: c.u8()? });
            }
            Msg::Snapshot { tick, people }
        }
        5 => Msg::Done,
        t => return Err(bad(&format!("unknown message {t}"))),
    };
    if c.i != body.len() {
        return Err(bad("trailing bytes"));
    }
    Ok(msg)
}

fn decode_command(c: &mut Cursor) -> io::Result<Command> {
    let tag = c.u8()?;
    let n = c.u8()?;
    let mut ids = Vec::with_capacity(n as usize);
    for _ in 0..n {
        ids.push(c.u16()?);
    }
    Ok(match tag {
        1 => Command::MoveSquad { ids, to: [c.f32()?, c.f32()?, c.f32()?] },
        2 => Command::Attack { ids, whom: c.u16()? },
        3 => Command::Stop { ids },
        t => return Err(bad(&format!("unknown command {t}"))),
    })
}

struct Cursor<'a> {
    b: &'a [u8],
    i: usize,
}

impl Cursor<'_> {
    fn take<const N: usize>(&mut self) -> io::Result<[u8; N]> {
        let s = self.b.get(self.i..self.i + N).ok_or_else(|| bad("message too short"))?;
        self.i += N;
        Ok(s.try_into().expect("length checked"))
    }
    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take::<1>()?[0])
    }
    fn u16(&mut self) -> io::Result<u16> {
        Ok(u16::from_le_bytes(self.take()?))
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take()?))
    }
    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(self.take()?))
    }
    fn f32(&mut self) -> io::Result<f32> {
        Ok(f32::from_le_bytes(self.take()?))
    }
}

fn bad(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_message_survives_the_wire() {
        let msgs = [
            Msg::Hello { version: VERSION, world: 0xdead_beef_0123 },
            Msg::Orders { tick: 7, commands: vec![] },
            Msg::Orders {
                tick: 9,
                commands: vec![
                    Command::MoveSquad { ids: vec![1, 2, 3], to: [1.5, -2.0, 3.25] },
                    Command::Attack { ids: vec![4], whom: 60 },
                    Command::Stop { ids: vec![5, 6] },
                ],
            },
            Msg::Hash { tick: 600, hash: u64::MAX },
            Msg::Snapshot {
                tick: 3,
                people: vec![PersonState { id: 2, position: [1.0, 2.0, 3.0], velocity: [0.5, 0.0, -0.5], flags: 1 }],
            },
            Msg::Done,
        ];
        for m in msgs {
            let bytes = encode(&m);
            assert_eq!(read_msg(&mut bytes.as_slice()).unwrap(), m);
        }
    }

    #[test]
    fn broken_bytes_are_an_error() {
        assert!(decode(&[]).is_err());
        assert!(decode(&[9]).is_err());
        assert!(decode(&[3, 1, 2]).is_err());
        let mut done = encode(&Msg::Done);
        done.push(0);
        done[..4].copy_from_slice(&2u32.to_le_bytes());
        assert!(read_msg(&mut done.as_slice()).is_err());
    }
}
