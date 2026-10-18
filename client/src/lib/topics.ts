/** MQTT topics, matching `probe_protocol::topics` in the Rust crate. */
export function mqttTopics(prefix: string, clientId: string) {
  return {
    request: `${prefix}/req/${clientId}`,
    response: `${prefix}/res/${clientId}`,
    events: `${prefix}/events/records`,
    serverStatus: `${prefix}/status/server`,
  };
}

/** `web-` plus 12 random hex characters: a valid single topic level. */
export function randomClientId(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(6));
  return `web-${Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("")}`;
}
