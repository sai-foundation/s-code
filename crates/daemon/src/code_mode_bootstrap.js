(function (send, emit, finish, names) {
  "use strict";
  const stringify = JSON.stringify;
  const parse = JSON.parse;
  const NativePromise = Promise;
  const pending = new Map();
  const mapSet = Function.prototype.call.bind(Map.prototype.set);
  const mapGet = Function.prototype.call.bind(Map.prototype.get);
  const mapDelete = Function.prototype.call.bind(Map.prototype.delete);
  let count = 0;
  const tools = Object.create(null);
  for (const name of names.split(",")) {
    tools[name] = (args) => new NativePromise((resolve, reject) => {
      const id = send(name, stringify(args));
      mapSet(pending, id, { resolve, reject });
      count++;
    });
  }
  Object.defineProperty(globalThis, "tools", { value: Object.freeze(tools) });
  Object.defineProperty(globalThis, "text", { value: (value) => emit(stringify(value ?? null)) });
  return {
    start: (program) => program().then(() => finish(null), (error) => finish(String(error))),
    deliver: (encoded) => {
      const response = parse(encoded);
      const call = mapGet(pending, response.id);
      if (!call) throw new Error("Unknown tool response");
      mapDelete(pending, response.id);
      count--;
      if (response.error !== undefined) call.reject(new Error(response.error));
      else call.resolve(response.value);
    },
    pending: () => count,
  };
})
