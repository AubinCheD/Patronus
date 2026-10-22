//! Patronus, sans interface : le format des tâches, leurs déclencheurs, leur exécution et la boîte
//! de résultats. L'application (`patronus-app`) n'est qu'un affichage par-dessus.

pub mod actions;
pub mod claude;
pub mod engine;
pub mod exec;
pub mod fsutil;
pub mod lock;
pub mod pricing;
pub mod remote;
pub mod run;
pub mod schedule;
pub mod task;
pub mod templates;
pub mod watch;
pub mod winproc;
pub mod winsched;
