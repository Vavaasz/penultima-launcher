use crate::constants::INSTANCE_CHECK_INTERVAL;
use crate::game_client::WindowState;
use crate::window_manager::WindowManager;
use anyhow::Result;
use log::info;
use single_instance::SingleInstance;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

pub struct InstanceManager {
    instance: Option<SingleInstance>,
    app_id: String,
}

impl InstanceManager {
    pub fn new(app_id: &str) -> Self {
        Self {
            instance: None,
            app_id: app_id.to_string(),
        }
    }

    pub fn ensure_single_instance(&mut self) -> Result<bool> {
        let instance = SingleInstance::new(&self.app_id)?;
        let is_single = instance.is_single();
        if is_single {
            self.instance = Some(instance);
        }
        Ok(is_single)
    }

    pub fn signal_running_instance(&self, data_dir: &PathBuf) -> Result<()> {
        let signal_file = data_dir.join("show.signal");
        info!(
            "Sinalizando para instÃ¢ncia existente atravÃ©s do arquivo: {:?}",
            signal_file
        );
        fs::write(signal_file, "show")?;
        Ok(())
    }

    pub fn start_signal_monitor(&self, data_dir: PathBuf, window_state: Arc<Mutex<WindowState>>) {
        thread::spawn(move || {
            let window_manager = WindowManager {
                window_state,
                needs_repaint: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            };

            loop {
                let signal_file = data_dir.join("show.signal");
                if signal_file.exists() {
                    info!("Sinal de mostrar janela detectado!");
                    let _ = fs::remove_file(&signal_file);
                    window_manager.show_window();
                }

                thread::sleep(INSTANCE_CHECK_INTERVAL);
            }
        });
    }
}
