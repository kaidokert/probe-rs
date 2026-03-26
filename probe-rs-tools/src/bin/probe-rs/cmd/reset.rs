use crate::{
    CoreOptions,
    rpc::client::RpcClient,
    util::{
        cli,
        common_options::{CliProtocol, ProbeOptions},
    },
};
use probe_rs::probe::{DebugProbeSelector, cmsisdap::reset_pkobn_updi_m4809};

#[derive(clap::Parser)]
pub struct Cmd {
    #[clap(flatten)]
    shared: CoreOptions,

    #[clap(flatten)]
    common: ProbeOptions,
}

impl Cmd {
    pub async fn run(self, client: RpcClient) -> anyhow::Result<()> {
        if self.common.protocol == Some(CliProtocol::Updi) {
            if !client.is_local_session() {
                anyhow::bail!(
                    "The protocol 'UPDI' is currently only supported by 'reset' in a local session."
                );
            }

            let probe =
                cli::select_probe(&client, self.common.probe.clone().map(Into::into)).await?;
            let selector: DebugProbeSelector = probe.selector().into();
            reset_pkobn_updi_m4809(&selector)?;
        } else {
            let session = cli::attach_probe(&client, self.common, false).await?;
            let core = session.core(self.shared.core);

            core.reset().await?;
        }

        Ok(())
    }
}
