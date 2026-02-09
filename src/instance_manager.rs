use crate::game_client::WindowState;
use crate::window_manager::WindowManager;
use anyhow::Result;
use single_instance::SingleInstance;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use log::info;

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

    // Tenta criar uma instância única do aplicativo
    pub fn ensure_single_instance(&mut self) -> Result<bool> {
        let instance = SingleInstance::new(&self.app_id)?;

        let is_single = instance.is_single();
        if is_single {
            self.instance = Some(instance);
        }

        Ok(is_single)
    }

    // Sinaliza para a instância em execução mostrar a janela
    pub fn signal_running_instance(&self, data_dir: &PathBuf) -> Result<()> {
        let signal_file = data_dir.join("show.signal");
        info!(
            "Sinalizando para instância existente através do arquivo: {:?}",
            signal_file
        );
        fs::write(signal_file, "show")?;
        Ok(())
    }

    // Monitora o sinal para mostrar a janela
    pub fn start_signal_monitor(&self, data_dir: PathBuf, window_state: Arc<Mutex<WindowState>>) {
        let window_state_monitor = window_state.clone();

        thread::spawn(move || {
            // Criar uma instância de WindowManager para usar seu método show_window
            let window_manager = WindowManager {
                window_state: window_state_monitor.clone(),
                needs_repaint: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            };

            loop {
                // Verifica se há tentativa de nova instância a cada segundo
                let should_check = {
                    let state = window_state_monitor.lock().unwrap();
                    state.last_check.elapsed() >= Duration::from_secs(1)
                };

                if should_check {
                    // Atualiza o timestamp fora do bloco principal para evitar deadlock
                    {
                        let mut state = window_state_monitor.lock().unwrap();
                        state.last_check = Instant::now();
                    }

                    let signal_file = data_dir.join("show.signal");
                    if signal_file.exists() {
                        info!("Sinal de mostrar janela detectado!");
                        let _ = fs::remove_file(&signal_file);
                        
                        // Usar o método do WindowManager em vez da função independente
                        window_manager.show_window();
                    }
                }

                // Dorme por mais tempo para reduzir consumo de CPU
                thread::sleep(Duration::from_secs(2));
            }
        });
    }
}
