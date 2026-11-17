<script>
  import { chat } from "./store.svelte.js";
  import { formatRelative, shortHash } from "./util.js";

  let { onOpenPaths = null } = $props();
  let showServices = $state(false);
  let selected = $state(null);
  let copied = $state(false);

  const SIZE = 620;
  const CX = SIZE / 2;
  const CY = SIZE / 2;
  const INNER = 84;
  const OUTER = 258;
  const MAX_NODES = 300;

  function hashAngle(seed) {
    let hash = 2166136261;
    for (const char of String(seed)) {
      hash ^= char.charCodeAt(0);
      hash = Math.imul(hash, 16777619);
    }
    return ((hash >>> 0) / 4294967296) * Math.PI * 2;
  }

  function point(radius, angle) {
    return {
      x: CX + Math.cos(angle) * radius,
      y: CY + Math.sin(angle) * radius,
    };
  }

  /// Where a transport diamond belongs when several peers reach it: the
  /// circular mean of their spokes, so every relayed edge stays a spoke.
  function meanAngle(angles) {
    let x = 0;
    let y = 0;
    for (const angle of angles) {
      x += Math.cos(angle);
      y += Math.sin(angle);
    }
    return Math.atan2(y, x);
  }

  const model = $derived.by(() => {
    const mesh = chat.mesh;
    const allPaths = mesh?.peer_paths ?? [];
    const live = allPaths.filter((path) => path.state !== "unresponsive");
    const stale = allPaths.filter((path) => path.state === "unresponsive");
    const maxHops = Math.min(6, Math.max(1, ...live.map((path) => path.hops)));
    const ringFor = (hops) => INNER + ((OUTER - INNER) * Math.min(hops, maxHops)) / maxHops;

    const peers = [];
    const spokes = new Map();
    for (const path of live) {
      const angle = hashAngle(path.address + ":" + path.hops);
      peers.push({ ...path, angle, hops: path.hops, radius: ringFor(path.hops) });
      if (path.hops > 1) {
        if (!spokes.has(path.next_hop)) {
          spokes.set(path.next_hop, { angles: [], newest: null });
        }
        const spoke = spokes.get(path.next_hop);
        spoke.angles.push(angle);
        const seen = (path.age ?? 0) * -1000;
        if (spoke.newest === null || seen > spoke.newest) spoke.newest = seen;
      }
    }
    // A first-hop transport is one hop away, so it sits on the hop-1 ring, on
    // the spoke its peers use. Sharing the next hop shares the diamond.
    const transports = new Map();
    for (const [address, spoke] of spokes) {
      transports.set(address, {
        address,
        angle: meanAngle(spoke.angles),
        radius: ringFor(1),
        last_seen: spoke.newest === null ? null : Date.now() + spoke.newest,
      });
    }
    // Stable order: radius then angle, so a refresh never reorders the picture.
    peers.sort((a, b) => a.hops - b.hops || a.angle - b.angle);
    // The cap counts every drawn node: peers, their transports, unseen peers
    // and services. Peers keep their spokes even when sharing the outer ring.
    const transportBudget = Math.min(transports.size, MAX_NODES);
    const drawnPeers = peers.slice(0, Math.max(0, MAX_NODES - transportBudget));
    const overflow = peers.length - drawnPeers.length;
    const liveAddresses = new Set(live.map((path) => path.address));
    const peersLeft = Math.max(0, MAX_NODES - drawnPeers.length - transportBudget);

    const unseen = chat.peers
      .filter((peer) => peer.address && !liveAddresses.has(peer.address))
      .map((peer) => {
        const deadPath = stale.find((path) => path.address === peer.address);
        return {
          ...peer,
          angle: hashAngle(peer.address),
          radius: OUTER + 18,
          hops: deadPath?.hops ?? null,
          interface: deadPath?.interface ?? null,
          state: deadPath ? "unresponsive" : "unseen",
        };
      })
      .slice(0, peersLeft);

    const services = showServices
      ? (mesh?.other_services ?? [])
          .map((service) => ({
            ...service,
            angle: hashAngle(service.destination),
            radius: OUTER + 38,
          }))
          .slice(0, Math.max(0, MAX_NODES - drawnPeers.length - transportBudget - unseen.length))
      : [];

    const edges = [];
    for (const peer of drawnPeers) {
      if (peer.hops <= 1) {
        edges.push({
          key: "centre-" + peer.address,
          from: { x: CX, y: CY },
          to: point(peer.radius, peer.angle),
          label: peer.interface,
          kind: "direct",
          mid: point(peer.radius / 2, peer.angle),
        });
      } else {
        const transport = transports.get(peer.next_hop);
        if (!transport) continue;
        edges.push({
          key: "relay-" + peer.address,
          from: point(transport.radius, transport.angle),
          to: point(peer.radius, peer.angle),
          label: peer.interface,
          kind: "relayed",
          mid: {
            x: (point(transport.radius, transport.angle).x + point(peer.radius, peer.angle).x) / 2,
            y: (point(transport.radius, transport.angle).y + point(peer.radius, peer.angle).y) / 2,
          },
        });
      }
    }
    for (const transport of transports.values()) {
      edges.push({
        key: "trunk-" + transport.address,
        from: { x: CX, y: CY },
        to: point(transport.radius, transport.angle),
        label: null,
        kind: "trunk",
        mid: point(transport.radius / 2, transport.angle),
      });
    }

    return {
      peers: drawnPeers,
      transports: [...transports.values()],
      unseen,
      services,
      edges,
      overflow,
      maxHops,
      liveCount: live.length,
    };
  });

  function peerName(address) {
    const known = chat.peerByAddress(address);
    return known?.nickname || known?.announced_name || shortHash(address, 4);
  }

  function activatePeer(peer) {
    const known = chat.peerByAddress(peer.address ?? peer.id);
    if (known) {
      chat.openPeer(known.id).catch((err) => chat.toast(err.message));
      return;
    }
    selected = { kind: "peer", ...peer };
  }

  function activateTransport(transport) {
    const peer = model.peers.find((item) => item.next_hop === transport.address);
    selected = {
      kind: "transport",
      address: transport.address,
      hops: peer?.hops ? peer.hops - 1 : null,
      interface: peer?.interface ?? null,
      state: "relayed",
      age: peer?.age ?? null,
      last_seen: transport.last_seen,
    };
  }

  async function copy(value) {
    try {
      await navigator.clipboard.writeText(value);
      copied = true;
      setTimeout(() => (copied = false), 1600);
    } catch {
      chat.toast("Could not access the clipboard.");
    }
  }

  function onKey(event, action) {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      action();
    }
  }
</script>

<div class="net-tab net-map">
  <div class="net-toolbar">
    <label class="net-check">
      <input type="checkbox" bind:checked={showServices} />
      Show other services
    </label>
    <span class="net-total">
      {model.liveCount} live {model.liveCount === 1 ? "path" : "paths"}
      {#if model.maxHops > 1}· up to {model.maxHops} hops{/if}
    </span>
  </div>

  {#if !chat.meshUp}
    <p class="net-map-stale">
      The bridge is offline. This is the last known mesh; positions stay put until
      it reports again.
    </p>
  {/if}

  <div class="net-map-frame">
    <svg
      class="net-map-svg"
      viewBox={`0 0 ${SIZE} ${SIZE}`}
      role="img"
      aria-label="Reticulum mesh graph. A text alternative follows."
    >
      {#each Array.from({ length: model.maxHops }, (_, index) => index + 1) as hop (hop)}
        <circle
          class="map-ring"
          cx={CX}
          cy={CY}
          r={INNER + ((OUTER - INNER) * hop) / model.maxHops}
        />
        <text class="map-ring-label" x={CX + 4} y={CY - (INNER + ((OUTER - INNER) * hop) / model.maxHops) - 4}>
          {hop} {hop === 1 ? "hop" : "hops"}
        </text>
      {/each}

      {#each model.edges as edge (edge.key)}
        <line
          class="map-edge"
          data-kind={edge.kind}
          x1={edge.from.x}
          y1={edge.from.y}
          x2={edge.to.x}
          y2={edge.to.y}
        />
        {#if edge.label}
          <text class="map-edge-label" x={edge.mid.x} y={edge.mid.y}>{edge.label}</text>
        {/if}
      {/each}

      <g class="map-self">
        <circle cx={CX} cy={CY} r="15" />
        <text x={CX} y={CY + 30}>you</text>
      </g>

      {#each model.transports as transport (transport.address)}
        {@const position = point(transport.radius, transport.angle)}
        <g
          class="map-node transport"
          tabindex="0"
          role="link"
          aria-label={`Transport ${transport.address}`}
          onclick={() => activateTransport(transport)}
          onkeydown={(event) => onKey(event, () => activateTransport(transport))}
          transform={`translate(${position.x} ${position.y})`}
        >
          <rect x="-6" y="-6" width="12" height="12" transform="rotate(45)" />
          <circle class="map-hit" r="18" />
          <title>Transport {shortHash(transport.address)}</title>
          <text class="map-node-label" y="-10">{shortHash(transport.address, 4)}</text>
        </g>
      {/each}

      {#each model.peers as peer (peer.address)}
        {@const position = point(peer.radius, peer.angle)}
        <g
          class="map-node peer"
          tabindex="0"
          role="link"
          aria-label={`${peerName(peer.address)}, ${peer.hops} hops via ${peer.interface}`}
          onclick={() => activatePeer(peer)}
          onkeydown={(event) => onKey(event, () => activatePeer(peer))}
          transform={`translate(${position.x} ${position.y})`}
        >
          <circle r="8" />
          <circle class="map-hit" r="20" />
          <title>{peerName(peer.address)} · {peer.hops} hops · {peer.interface}</title>
          <text class="map-node-label" y="-13">{peerName(peer.address)}</text>
        </g>
      {/each}

      {#each model.unseen as peer (peer.address)}
        {@const position = point(peer.radius, peer.angle)}
        <g
          class="map-node peer unseen"
          tabindex="0"
          role="link"
          aria-label={`${peerName(peer.address)}, no live path`}
          onclick={() => activatePeer(peer)}
          onkeydown={(event) => onKey(event, () => activatePeer(peer))}
          transform={`translate(${position.x} ${position.y})`}
        >
          <circle r="7" />
          <circle class="map-hit" r="20" />
          <title>{peerName(peer.address)} · {peer.state === "unresponsive" ? "path is down" : "no live path"}</title>
          <text
            class="map-node-label"
            x={-Math.cos(peer.angle) * 14}
            y={-Math.sin(peer.angle) * 14}
          >{peerName(peer.address)}</text>
        </g>
      {/each}

      {#each model.services as service (service.destination)}
        {@const position = point(service.radius, service.angle)}
        <g
          class="map-node service"
          tabindex="0"
          role="link"
          aria-label={`Service ${service.aspect || shortHash(service.destination)}`}
          onclick={() => (selected = { kind: "service", ...service })}
          onkeydown={(event) => onKey(event, () => (selected = { kind: "service", ...service }))}
          transform={`translate(${position.x} ${position.y})`}
        >
          <circle r="4" />
          <circle class="map-hit" r="18" />
          <title>{service.aspect || shortHash(service.destination)}</title>
          <text
            class="map-node-label"
            x={-Math.cos(service.angle) * 14}
            y={-Math.sin(service.angle) * 14}
          >{service.aspect || shortHash(service.destination, 4)}</text>
        </g>
      {/each}
    </svg>

    {#if model.overflow > 0}
      <button class="net-map-more" onclick={onOpenPaths}>
        +{model.overflow} more on the Paths tab
      </button>
    {/if}
  </div>

  {#if model.peers.length === 0 && model.unseen.length === 0}
    <p class="net-empty">
      No announces yet. Peers appear here once they announce themselves.
    </p>
  {/if}

  {#if selected}
    <div class="net-popover" role="dialog" aria-label="Mesh node details">
      <div class="net-popover-head">
        <strong>{selected.kind === "transport" ? "Transport node" : selected.kind === "service" ? "Service" : "Peer"}</strong>
        <button class="icon-button" title="Close" onclick={() => (selected = null)}>×</button>
      </div>
      <dl class="net-detail">
        <dt>Address</dt>
        <dd class="mono break">{selected.address || selected.destination}</dd>
        {#if selected.hops != null}
          <dt>Hops</dt>
          <dd>{selected.hops}</dd>
        {/if}
        {#if selected.interface}
          <dt>Interface</dt>
          <dd>{selected.interface}</dd>
        {/if}
        {#if selected.aspect}
          <dt>Aspect</dt>
          <dd>{selected.aspect}</dd>
        {/if}
        {#if selected.last_seen}
          <dt>Last seen</dt>
          <dd>{formatRelative(selected.last_seen)}</dd>
        {/if}
      </dl>
      <button
        class="btn ghost"
        onclick={() => copy(selected.address || selected.destination)}
      >
        {copied ? "Copied" : "Copy address"}
      </button>
    </div>
  {/if}

  <ul class="sr-only">
    <li>You are at the centre of the graph.</li>
    {#each model.peers as peer (peer.address)}
      <li>
        {peerName(peer.address)}: {peer.hops} {peer.hops === 1 ? "hop" : "hops"} away,
        via {peer.interface}
        {#if peer.hops > 1}, after transport {shortHash(peer.next_hop)}{/if}.
      </li>
    {/each}
    {#each model.unseen as peer (peer.address)}
      <li>{peerName(peer.address)}: no live path; last seen {formatRelative(peer.last_seen)}.</li>
    {/each}
    {#each model.transports as transport (transport.address)}
      <li>Transport node {shortHash(transport.address)} carries relayed paths.</li>
    {/each}
  </ul>
</div>
