use super::proc::{Proc};
use super::raw_types;
use super::value::Value;
use super::DMContext;
use crate::raw_types::values::IntoRawValue;
use crate::runtime::DMResult;
use detour::RawDetour;
use std::cell::RefCell;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::Once;

pub struct CompileTimeHook {
	pub proc_path: &'static str,
	pub hook: ProcHook,
}

impl CompileTimeHook {
	pub fn new(proc_path: &'static str, hook: ProcHook) -> Self {
		CompileTimeHook { proc_path, hook }
	}
}

inventory::collect!(CompileTimeHook);

extern "C" {

	static mut call_proc_by_id_original: *const ();

	fn call_proc_by_id_original_trampoline(
		usr: raw_types::values::Value,
		proc_type: u32,
		proc_id: raw_types::procs::ProcId,
		unk_0: u32,
		src: raw_types::values::Value,
		args: *mut raw_types::values::Value,
		args_countL: usize,
		unk_1: u32,
		unk_2: u32,
	) -> raw_types::values::Value;

	fn call_proc_by_id_hook_trampoline(
		usr: raw_types::values::Value,
		proc_type: u32,
		proc_id: raw_types::procs::ProcId,
		unk_0: u32,
		src: raw_types::values::Value,
		args: *mut raw_types::values::Value,
		args_countL: usize,
		unk_1: u32,
		unk_2: u32,
	) -> raw_types::values::Value;
}

pub enum HookFailure {
	NotInitialized,
	ProcNotFound,
	AlreadyHooked,
	UnknownFailure,
}

impl std::fmt::Debug for HookFailure {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::NotInitialized => write!(f, "Library not initialized"),
			Self::ProcNotFound => write!(f, "Proc not found"),
			Self::AlreadyHooked => write!(f, "Proc is already hooked"),
			Self::UnknownFailure => write!(f, "Unknown failure"),
		}
	}
}

pub fn init() -> Result<(), String> {
	// TODO: clean up and pass on errors
	unsafe {
		let hook = RawDetour::new(
			raw_types::funcs::call_proc_by_id_byond as *const (),
			call_proc_by_id_hook_trampoline as *const (),
		)
		.unwrap();

		hook.enable().unwrap();
		call_proc_by_id_original = std::mem::transmute(hook.trampoline());
		std::mem::forget(hook);
	}
	Ok(())
}

pub type ProcHook =
	for<'a, 'r> fn(&'a DMContext<'r>, Value<'a>, Value<'a>, &mut Vec<Value<'a>>) -> DMResult<'a>;

thread_local! {
	static PROC_HOOKS: RefCell<HashMap<raw_types::procs::ProcId, ProcHook>> = RefCell::new(HashMap::new());
}

static PROC_HOOKS_INIT: Once = Once::new();

fn hook_by_id(id: raw_types::procs::ProcId, hook: ProcHook) -> Result<(), HookFailure> {
	PROC_HOOKS_INIT.call_once(|| {
		if let Err(e) = init() {
			panic!(e);
		}
	});
	PROC_HOOKS.with(|h| {
		let mut map = h.borrow_mut();
		match map.entry(id) {
			Entry::Vacant(v) => {
				v.insert(hook);
				Ok(())
			}
			Entry::Occupied(_) => Err(HookFailure::AlreadyHooked),
		}
	})
}

pub fn hook<S: Into<String>>(name: S, hook: ProcHook) -> Result<(), HookFailure> {
	match super::proc::get_proc(name) {
		Some(p) => hook_by_id(p.id, hook),
		None => Err(HookFailure::ProcNotFound),
	}
}

impl Proc {
	#[allow(unused)]
	pub fn hook(&self, func: ProcHook) -> Result<(), HookFailure> {
		hook_by_id(self.id, func)
	}
}

#[no_mangle]
extern "C" fn call_proc_by_id_hook(
	usr_raw: raw_types::values::Value,
	proc_type: u32,
	proc_id: raw_types::procs::ProcId,
	unknown1: u32,
	src_raw: raw_types::values::Value,
	args_ptr: *mut raw_types::values::Value,
	num_args: usize,
	unknown2: u32,
	unknown3: u32,
) -> raw_types::values::Value {
	return PROC_HOOKS.with(|h| match h.borrow().get(&proc_id) {
		Some(hook) => {
			let ctx = DMContext::new().unwrap();
			let src;
			let usr;
			let mut args: Vec<Value>;

			unsafe {
				// TODO: ref count check
				src = Value::from_raw(src_raw);
				usr = Value::from_raw(usr_raw);

				// Taking ownership of args here
				args = std::slice::from_raw_parts(args_ptr, num_args)
					.iter()
					.map(|v| Value::from_raw_owned(*v))
					.collect();
			}

			let result = hook(&ctx, src, usr, &mut args);

			match result {
				Ok(r) => {
					let result_raw = unsafe { (&r).into_raw_value() };
					// Stealing our reference out of the Value
					std::mem::forget(r);
					result_raw
				}
				Err(e) => {
					msgbox::create("Wtf bro?", e.message.as_str(), msgbox::IconType::Error);
					unsafe { Value::null().into_raw_value() }
				}
			}
		}
		None => unsafe {
			call_proc_by_id_original_trampoline(
				usr_raw, proc_type, proc_id, unknown1, src_raw, args_ptr, num_args, unknown2,
				unknown3,
			)
		},
	});
}
