"use strict";

function validatedDaemonBase(value) {
  let url;
  try {
    url = new URL(value);
  } catch {
    throw new Error("Daemon URL must be a valid loopback HTTP origin.");
  }
  const loopbackHosts = new Set(["localhost", "127.0.0.1", "[::1]"]);
  if (
    url.protocol !== "http:" ||
    !loopbackHosts.has(url.hostname) ||
    !url.port ||
    url.username ||
    url.password ||
    (url.pathname !== "/" && url.pathname !== "") ||
    url.search ||
    url.hash
  ) {
    throw new Error("Daemon URL must be an HTTP loopback origin with an explicit port and no credentials or path.");
  }
  return url.origin;
}

function browserBootstrapUrl(base, token) {
  const url = new URL(validatedDaemonBase(base));
  url.hash = new URLSearchParams({ "opencoding-bootstrap": token }).toString();
  return url.toString();
}

module.exports = { validatedDaemonBase, browserBootstrapUrl };
