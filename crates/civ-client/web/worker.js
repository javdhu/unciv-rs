// One engine thread (src/thread.rs): instantiate the page's module on its shared memory, then run the task.
self.onmessage = async ({ data: [module, memory, task] }) => {
  const pkg = await import('./civ_client.js');
  // civ-core's AI recurses deeper than wasm-bindgen's default thread stack.
  await pkg.default({ module_or_path: module, memory, thread_stack_size: 8 << 20 });
  pkg.civ_worker_entry(task);
};
