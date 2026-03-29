use crate::rpc::{
    Key,
    functions::{NoResponse, RpcContext, RpcResult},
};
use postcard_rpc::header::VarHeader;
use postcard_schema::Schema;
use probe_rs::{
    MemoryInterface, Session, probe::cmsisdap::AvrMemoryRegion as ProbeRsAvrMemoryRegion,
};
use serde::{Deserialize, Serialize};

pub trait Word: Copy + Default + Send + Schema {
    fn write(core: &mut impl MemoryInterface, address: u64, data: &[Self]) -> anyhow::Result<()>;
    fn encode(words: &[Self]) -> Vec<u8>;
}

impl Word for u8 {
    fn write(core: &mut impl MemoryInterface, address: u64, data: &[Self]) -> anyhow::Result<()> {
        core.write_8(address, data)?;
        Ok(())
    }

    fn encode(words: &[Self]) -> Vec<u8> {
        words.to_vec()
    }
}
impl Word for u16 {
    fn write(core: &mut impl MemoryInterface, address: u64, data: &[Self]) -> anyhow::Result<()> {
        core.write_16(address, data)?;
        Ok(())
    }

    fn encode(words: &[Self]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_le_bytes()).collect()
    }
}
impl Word for u32 {
    fn write(core: &mut impl MemoryInterface, address: u64, data: &[Self]) -> anyhow::Result<()> {
        core.write_32(address, data)?;
        Ok(())
    }

    fn encode(words: &[Self]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_le_bytes()).collect()
    }
}
impl Word for u64 {
    fn write(core: &mut impl MemoryInterface, address: u64, data: &[Self]) -> anyhow::Result<()> {
        core.write_64(address, data)?;
        Ok(())
    }

    fn encode(words: &[Self]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_le_bytes()).collect()
    }
}

#[derive(Serialize, Deserialize, Schema)]
pub struct WriteMemoryRequest<W: Word> {
    pub sessid: Key<Session>,
    pub core: u32,
    pub address: u64,
    pub data: Vec<W>,
    pub region: Option<AvrMemoryRegion>,
}

pub async fn write_memory<W: Word + 'static>(
    ctx: &mut RpcContext,
    _header: VarHeader,
    request: WriteMemoryRequest<W>,
) -> NoResponse {
    let mut session = ctx.session(request.sessid).await;
    if session.architecture() == probe_rs::Architecture::Avr {
        session.write_memory(
            request.core as usize,
            request.address,
            std::mem::size_of::<W>(),
            &W::encode(&request.data),
            request.region.map(Into::into),
        )?;
    } else {
        let mut core = session.core(request.core as usize)?;
        W::write(&mut core, request.address, &request.data)?;
    }
    Ok(())
}

#[derive(Serialize, Deserialize, Schema)]
pub struct ReadMemoryRequest {
    pub sessid: Key<Session>,
    pub core: u32,
    pub address: u64,
    pub count: u32,
    pub region: Option<AvrMemoryRegion>,
}

pub async fn read_memory<W: Word + 'static>(
    ctx: &mut RpcContext,
    _header: VarHeader,
    request: ReadMemoryRequest,
) -> RpcResult<Vec<W>> {
    let mut session = ctx.session(request.sessid).await;
    let bytes = session.read_memory(
        request.core as usize,
        request.address,
        std::mem::size_of::<W>(),
        request.count as usize,
        request.region.map(Into::into),
    )?;

    let expected_len = request.count as usize * std::mem::size_of::<W>();
    if bytes.len() < expected_len {
        return Err(anyhow::anyhow!(
            "read_memory returned {} bytes, expected {}",
            bytes.len(),
            expected_len
        )
        .into());
    }
    let mut words = vec![W::default(); request.count as usize];
    for (index, word) in words.iter_mut().enumerate() {
        let start = index * std::mem::size_of::<W>();
        let end = start + std::mem::size_of::<W>();
        *word = read_word::<W>(&bytes[start..end])?;
    }
    Ok(words)
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Schema)]
pub enum AvrMemoryRegion {
    Flash,
    Eeprom,
    Fuses,
    Lock,
    UserRow,
    ProdSig,
}

impl From<AvrMemoryRegion> for ProbeRsAvrMemoryRegion {
    fn from(region: AvrMemoryRegion) -> Self {
        match region {
            AvrMemoryRegion::Flash => ProbeRsAvrMemoryRegion::Flash,
            AvrMemoryRegion::Eeprom => ProbeRsAvrMemoryRegion::Eeprom,
            AvrMemoryRegion::Fuses => ProbeRsAvrMemoryRegion::Fuses,
            AvrMemoryRegion::Lock => ProbeRsAvrMemoryRegion::Lock,
            AvrMemoryRegion::UserRow => ProbeRsAvrMemoryRegion::UserRow,
            AvrMemoryRegion::ProdSig => ProbeRsAvrMemoryRegion::ProdSig,
        }
    }
}

fn read_word<W: Word + 'static>(bytes: &[u8]) -> anyhow::Result<W> {
    let any: Box<dyn std::any::Any> = match bytes.len() {
        1 => Box::new(bytes[0]),
        2 => Box::new(u16::from_le_bytes([bytes[0], bytes[1]])),
        4 => Box::new(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])),
        8 => Box::new(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ])),
        _ => anyhow::bail!("unsupported word width {}", bytes.len()),
    };

    any.downcast::<W>()
        .map(|boxed| *boxed)
        .map_err(|_| anyhow::anyhow!("failed to decode read word"))
}
