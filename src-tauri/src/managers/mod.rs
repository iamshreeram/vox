pub mod acp_agent;
#[cfg(test)]
mod acp_agent_permission_tests;
#[cfg(test)]
mod acp_agent_tests;
#[cfg(test)]
mod acp_allow_all_tests;
pub mod acp_escalation;
#[cfg(test)]
mod acp_escalation_tests;
#[cfg(test)]
mod acp_latch_tests;
pub mod acp_permission;
pub mod acp_permission_broker;
#[cfg(test)]
mod acp_permission_broker_tests;
#[cfg(test)]
mod acp_permission_tests;
pub mod acp_permission_ui;
#[cfg(test)]
mod acp_permission_ui_tests;
pub mod acp_run;
#[cfg(test)]
mod acp_run_tests;
pub mod agent_jobs;
pub mod agent_jobs_output;
pub mod agent_settings_gate;
#[cfg(test)]
mod agent_settings_gate_tests;
pub mod agent_tools;
pub mod ambient;
pub mod ambient_listener;
pub mod audio;
pub mod aux_model;
pub mod brain;
#[cfg(test)]
mod brain_store_tests;
#[cfg(test)]
mod brain_tests;
pub mod chat_log;
#[cfg(test)]
mod chat_log_tests;
pub mod chat_tap;
#[cfg(test)]
mod chat_tap_tests;
pub mod command_router;
pub mod composer_config;
pub mod composer_rules;
pub mod conversation;
pub mod conversation_record;
#[cfg(test)]
mod conversation_record_tests;
pub mod conversation_save;
#[cfg(test)]
mod conversation_save_tests;
pub mod conversation_store;
#[cfg(test)]
mod conversation_store_tests;
pub mod converse;
pub mod converse_ack;
pub mod converse_agent;
pub mod converse_audio;
pub mod converse_barge;
#[cfg(test)]
mod converse_barge_loop_tests;
pub mod converse_barge_monitor;
#[cfg(test)]
mod converse_barge_monitor_tests;
pub mod converse_barge_platform;
#[cfg(test)]
mod converse_barge_platform_tests;
#[cfg(test)]
mod converse_barge_tests;
pub mod converse_capture;
pub mod converse_confirm;
pub mod converse_context;
pub mod converse_hooks;
pub mod converse_latency;
pub mod converse_local;
pub mod converse_overlay;
pub mod converse_plan;
#[cfg(test)]
mod converse_plan_vault_tests;
#[cfg(test)]
mod converse_planner_launch_tests;
#[cfg(test)]
mod converse_planner_loop_tests;
#[cfg(test)]
mod converse_planner_wiring_tests;
pub mod converse_platform;
pub mod converse_queue;
#[cfg(test)]
mod converse_rag_probe_tests;
#[cfg(test)]
mod converse_rag_tests;
pub mod converse_role;
pub mod converse_runtime;
pub mod converse_stream;
pub mod converse_text;
pub mod emotion;
pub mod gguf_meta;
pub mod history;
pub mod job_announcer;
pub mod job_speaker;
pub mod job_summary;
#[cfg(test)]
mod live_acceptance_tests;
pub mod llm_stream;
pub mod llm_tools;
pub mod media_control;
pub mod memory;
pub mod model;
pub mod model_capabilities;
pub mod natural_audio;
#[cfg(test)]
mod natural_audio_tests;
pub mod natural_chunk;
#[cfg(test)]
mod natural_chunk_tests;
#[cfg(test)]
mod natural_duck_tests;
pub mod natural_engine;
#[cfg(test)]
mod natural_engine_tests;
#[cfg(target_os = "macos")]
pub mod natural_kokoro;
#[cfg(all(test, target_os = "macos"))]
mod natural_kokoro_tests;
#[cfg(test)]
mod natural_lang_tests;
pub mod natural_model;
#[cfg(test)]
mod natural_model_tests;
#[cfg(target_os = "macos")]
pub mod natural_phonon;
pub mod natural_phonon_core;
#[cfg(test)]
mod natural_phonon_core_tests;
#[cfg(all(test, target_os = "macos"))]
mod natural_phonon_e2e_tests;
#[cfg(test)]
mod natural_phonon_loader_tests;
#[cfg(test)]
mod natural_phonon_model_tests;
#[cfg(all(test, target_os = "macos"))]
mod natural_phonon_tests;
pub mod natural_router;
#[cfg(test)]
mod natural_router_phonon_tests;
#[cfg(test)]
mod natural_router_tests;
pub mod natural_sink;
#[cfg(test)]
mod natural_sink_tests;
pub mod natural_snapshot;
#[cfg(test)]
mod natural_test_support;
pub mod natural_text;
#[cfg(test)]
mod natural_text_tests;
pub mod natural_voices;
pub mod ort_sessions;
pub mod ownership;
pub mod persona;
mod plan_clock;
pub mod realtime_complete;
pub mod realtime_config;
pub mod redact;
#[cfg(test)]
mod redact_tests;
pub mod safety_policy;
pub mod smart_turn;
pub mod source_update;
pub mod speaker_enroll;
pub mod speaker_enroll_adapters;
pub mod speaker_id;
pub mod speech_backends;
pub mod speech_compose;
pub mod speech_pipeline;
pub mod tool_loop;
pub mod transcription;
pub mod tts;
#[cfg(target_os = "macos")]
pub mod tts_av;
pub mod tts_engines;
#[cfg(test)]
mod tts_engines_tests;
pub mod tts_queue;
pub mod tts_serial;
#[cfg(test)]
mod tts_serial_tests;
pub mod tts_voices;
pub mod vault;
pub mod vault_context;
#[cfg(test)]
mod vault_context_tests;
pub mod vault_index;
#[cfg(test)]
mod vault_index_tests;
pub mod vault_io;
#[cfg(test)]
mod vault_io_tests;
#[cfg(test)]
mod vault_query_tests;
pub mod vault_rag;
pub mod vault_rag_runtime;
#[cfg(test)]
mod vault_rag_runtime_tests;
#[cfg(test)]
mod vault_rag_settings_tests;
#[cfg(test)]
mod vault_rag_tests;
#[cfg(test)]
mod vault_rag_tools_tests;
#[cfg(test)]
mod vault_tests;
#[cfg(test)]
mod vault_walk_tests;
pub mod voice_adapters;
pub mod voice_common;
pub mod voice_models;
pub mod voice_runtime;
pub mod voice_shortcuts;
#[cfg(test)]
mod wake_conversation_tests;
pub mod wakeword;
pub mod wakeword_listener;
