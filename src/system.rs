use anyhow::Result;
use log::info;
use windows::Win32::System::Threading::{
    GetCurrentProcess, HIGH_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, SetPriorityClass,
};

/// Configura a prioridade do processo no Windows
pub fn set_process_priority(high_priority: bool) -> Result<()> {
    unsafe {
        let handle = GetCurrentProcess();
        let priority = if high_priority {
            HIGH_PRIORITY_CLASS
        } else {
            NORMAL_PRIORITY_CLASS
        };

        match SetPriorityClass(handle, priority) {
            Ok(_) => {
                info!(
                    "[SISTEMA] Prioridade do processo configurada: {}",
                    if high_priority { "Alta" } else { "Normal" }
                );
                Ok(())
            }
            Err(e) => Err(anyhow::Error::new(e)),
        }
    }
}
