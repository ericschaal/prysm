use tokio::sync::watch::Sender;
use tokio::task::JoinHandle;

pub struct CtrlCHandler(Sender<bool>);


impl CtrlCHandler {

    pub fn new(sender: Sender<bool>) -> Self {
        CtrlCHandler(sender)
    }
    pub fn start(&self) -> JoinHandle<()> {
        let watch = self.0.clone();
        tokio::spawn(async move {
            match tokio::signal::ctrl_c().await {
                Ok(()) => {
                    tracing::info!("Received Ctrl+C, initiating shutdown...");
                    let _ = watch.send(true);
                }
                Err(err) => {
                    tracing::error!("Failed to listen for Ctrl+C: {}", err);
                }
            }
        })
    }
}