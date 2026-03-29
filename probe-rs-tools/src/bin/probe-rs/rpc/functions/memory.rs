use crate::rpc::{
    Key,
    functions::{NoResponse, RpcContext, RpcResult},
};
use postcard_rpc::header::VarHeader;
use postcard_schema::Schema;
use probe_rs::{MemoryInterface, Session};
use serde::{Deserialize, Serialize};

pub trait Word: Copy + Default + Send + Schema {
    fn read(
        core: &mut impl MemoryInterface,
        address: u64,
        out: &mut Vec<Self>,
    ) -> anyhow::Result<()>;

    fn write(core: &mut impl MemoryInterface, address: u64, data: &[Self]) -> anyhow::Result<()>;
}

impl Word for u8 {
    fn read(
        core: &mut impl MemoryInterface,
        address: u64,
        out: &mut Vec<Self>,
    ) -> anyhow::Result<()> {
        core.read_8(address, out)?;
        Ok(())
    }

    fn write(core: &mut impl MemoryInterface, address: u64, data: &[Self]) -> anyhow::Result<()> {
        core.write_8(address, data)?;
        Ok(())
    }
}
impl Word for u16 {
    fn read(
        core: &mut impl MemoryInterface,
        address: u64,
        out: &mut Vec<Self>,
    ) -> anyhow::Result<()> {
        core.read_16(address, out)?;
        Ok(())
    }

    fn write(core: &mut impl MemoryInterface, address: u64, data: &[Self]) -> anyhow::Result<()> {
        core.write_16(address, data)?;
        Ok(())
    }
}
impl Word for u32 {
    fn read(
        core: &mut impl MemoryInterface,
        address: u64,
        out: &mut Vec<Self>,
    ) -> anyhow::Result<()> {
        core.read_32(address, out)?;
        Ok(())
    }

    fn write(core: &mut impl MemoryInterface, address: u64, data: &[Self]) -> anyhow::Result<()> {
        core.write_32(address, data)?;
        Ok(())
    }
}
impl Word for u64 {
    fn read(
        core: &mut impl MemoryInterface,
        address: u64,
        out: &mut Vec<Self>,
    ) -> anyhow::Result<()> {
        core.read_64(address, out)?;
        Ok(())
    }

    fn write(core: &mut impl MemoryInterface, address: u64, data: &[Self]) -> anyhow::Result<()> {
        core.write_64(address, data)?;
        Ok(())
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
    let address = avr_resolve_address(&session, request.address, request.region)?;
    let mut core = session.core(request.core as usize)?;
    W::write(&mut core, address, &request.data)?;
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
    let address = avr_resolve_address(&session, request.address, request.region)?;
    let mut core = session.core(request.core as usize)?;
    let mut words = vec![W::default(); request.count as usize];
    W::read(&mut core, address, &mut words)?;
    Ok(words)
}

/// AVR UPDI memory region selector, used by the CLI `read`/`write` commands
/// to address specific memory regions by name rather than absolute address.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Schema)]
pub enum AvrMemoryRegion {
    Flash,
    Eeprom,
    Fuses,
    Lock,
    UserRow,
    ProdSig,
}

/// Convert a region-relative AVR address to an absolute address.
///
/// For non-AVR sessions (or when no region is specified), the address is
/// returned unchanged. When a region is specified for an AVR session,
/// the region's base address from the chip descriptor is added to the
/// user-supplied offset.
fn avr_resolve_address(
    session: &Session,
    address: u64,
    region: Option<AvrMemoryRegion>,
) -> anyhow::Result<u64> {
    let Some(region) = region else {
        return Ok(address);
    };
    let chip = session
        .avr_chip_descriptor()
        .ok_or_else(|| anyhow::anyhow!("AVR region specified but session is not AVR"))?;
    let base: u64 = match region {
        // Flash region: the MemoryInterface uses 0-based flash offsets
        // (the Avr core's address_to_region maps [0..flash_size) to Flash).
        AvrMemoryRegion::Flash => 0,
        AvrMemoryRegion::Eeprom => chip.eeprom_base.into(),
        AvrMemoryRegion::Fuses => chip.fuses_base.into(),
        AvrMemoryRegion::Lock => chip.lock_base.into(),
        AvrMemoryRegion::UserRow => chip.userrow_base.into(),
        AvrMemoryRegion::ProdSig => chip.signature_base.into(),
    };
    Ok(base + address)
}
