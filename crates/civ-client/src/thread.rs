// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One way to start a thread on both targets.
//!
//! Native uses `std::thread`. On wasm `std::thread::spawn` panics, so a module Web Worker instantiates this
//! same module on the page's shared memory and runs the task (`web/worker.js`). The page names that script in
//! `globalThis.civWorkerUrl`.

pub fn spawn(name: &str, task: impl FnOnce() + Send + 'static) {
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::Builder::new()
        .name(name.into())
        .spawn(task)
        .expect("start a thread");
    #[cfg(target_arch = "wasm32")]
    web::spawn(name, Box::new(task));
}

/// Whether this instance runs in a worker, where the app itself must not start.
pub fn in_worker() -> bool {
    #[cfg(target_arch = "wasm32")]
    return web::in_worker();
    #[cfg(not(target_arch = "wasm32"))]
    false
}

#[cfg(target_arch = "wasm32")]
mod web {
    use wasm_bindgen::prelude::*;

    type Task = Box<dyn FnOnce() + Send>;

    /// Called by `web/worker.js` once the worker's instance is ready.
    #[wasm_bindgen]
    pub fn civ_worker_entry(task: u32) {
        // SAFETY: `task` is the pointer `spawn` leaked for exactly this worker, taken back once.
        let task = unsafe { Box::from_raw(task as *mut Task) };
        task();
    }

    pub fn spawn(name: &str, task: Task) {
        let task = Box::into_raw(Box::new(task)) as u32;
        let url = js_sys::Reflect::get(&js_sys::global(), &"civWorkerUrl".into())
            .ok()
            .and_then(|url| url.as_string())
            .expect("the page sets civWorkerUrl");
        let options = web_sys::WorkerOptions::new();
        options.set_type(web_sys::WorkerType::Module);
        options.set_name(name);
        let worker = web_sys::Worker::new_with_options(&url, &options).expect("start a worker");
        let message = js_sys::Array::of3(&wasm_bindgen::module(), &wasm_bindgen::memory(), &task.into());
        worker.post_message(&message).expect("hand the worker its task");
        // The worker runs for the life of the page.
        std::mem::forget(worker);
    }

    pub fn in_worker() -> bool {
        js_sys::global().dyn_ref::<web_sys::DedicatedWorkerGlobalScope>().is_some()
    }
}
