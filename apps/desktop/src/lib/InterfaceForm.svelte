<script>
  import { onMount } from "svelte";
  import { chat } from "./store.svelte.js";

  let { type, row = null, existingNames = [], onSaved, onCancel } = $props();

  const fields = $derived(type?.fields ?? []);
  const serialFields = $derived(fields.some((field) => field.widget === "serial"));

  let name = $state(suggestName());
  let values = $state(initialValues());
  let unknown = $state(initialUnknown());
  let revealed = $state({});
  let advancedOpen = $state(Object.keys(initialUnknown()).length > 0);
  let error = $state("");
  let errorField = $state("");
  let busy = $state(false);

  function suggestName() {
    if (row?.name) return row.name;
    const base = type?.label || type?.kind || "Interface";
    if (!existingNames.includes(base)) return base;
    for (let index = 2; index < 50; index += 1) {
      const candidate = `${base} ${index}`;
      if (!existingNames.includes(candidate)) return candidate;
    }
    return `${base} ${Date.now() % 1000}`;
  }

  function unitFactor(field) {
    if (field.units === "MHz") return 1_000_000;
    if (field.units === "kHz") return 1_000;
    return 1;
  }

  function initialValues() {
    const config = row?.config ?? {};
    const out = {};
    for (const field of fields) {
      let value = config[field.key];
      if (value === undefined) {
        value =
          field.default !== undefined
            ? field.default
            : field.widget === "bool"
              ? false
              : "";
      }
      if ((field.widget === "int" || field.widget === "port") && typeof value === "number") {
        value = value / unitFactor(field);
      }
      // Imported radio configs keep numbers in `<select>` keys (for example
      // `spreadingfactor = 8`); option values are strings, so match them.
      if (
        (field.widget === "select" || field.widget === "serial") &&
        value !== undefined &&
        value !== null &&
        value !== ""
      ) {
        value = String(value);
      }
      out[field.key] = value;
    }
    return out;
  }

  function initialUnknown() {
    const config = row?.config ?? {};
    const known = new Set(fields.map((field) => field.key));
    const out = {};
    for (const [key, value] of Object.entries(config)) {
      if (!known.has(key)) out[key] = value;
    }
    return out;
  }

  function visible(field) {
    if (!field.show_if) return true;
    return String(values[field.show_if.key]) === String(field.show_if.equals);
  }

  function coerce(field, value) {
    if (field.widget === "bool") return Boolean(value);
    if (field.widget === "int" || field.widget === "port") {
      const number = Number(value);
      if (!Number.isFinite(number)) return value;
      return Math.round(number * unitFactor(field));
    }
    return value;
  }

  function guessValue(value) {
    if (typeof value === "boolean") return value;
    if (value === "true") return true;
    if (value === "false") return false;
    if (/^-?\d+$/.test(String(value))) return Number(value);
    return value;
  }

  function buildConfig() {
    const config = {};
    for (const field of fields) {
      if (!visible(field)) continue;
      const value = values[field.key];
      if (value === "" || value === null || value === undefined) continue;
      config[field.key] = coerce(field, value);
    }
    for (const [key, value] of Object.entries(unknown)) {
      const trimmed = key.trim();
      if (!trimmed || value === "") continue;
      config[trimmed] = guessValue(value);
    }
    return config;
  }

  function validate() {
    errorField = "";
    if (!row) {
      const trimmed = name.trim();
      if (!trimmed) {
        error = "Give the interface a name.";
        return false;
      }
      if (trimmed.length > 64) {
        error = "Interface names are limited to 64 characters.";
        return false;
      }
      if (/[[\]\r\n]/.test(trimmed)) {
        error = "Interface names cannot contain [ ], or line breaks.";
        return false;
      }
      if (existingNames.includes(trimmed)) {
        error = `An interface named "${trimmed}" already exists.`;
        return false;
      }
    }
    for (const field of fields) {
      if (!visible(field) || !field.required) continue;
      const value = values[field.key];
      if (value === "" || value === null || value === undefined) {
        error = `${field.label} is required.`;
        errorField = field.key;
        if (!field.common) advancedOpen = true;
        return false;
      }
    }
    return true;
  }

  async function save() {
    error = "";
    if (!validate()) return;
    busy = true;
    try {
      const config = buildConfig();
      const result = row
        ? await chat.updateInterface(row.id, config)
        : await chat.createInterface(name.trim(), type.kind, config);
      onSaved?.(result);
    } catch (err) {
      error = err?.message ?? String(err);
      const lowered = error.toLowerCase();
      const hit = fields.find(
        (field) => field.key && lowered.includes(field.key.toLowerCase()),
      );
      if (hit) {
        errorField = hit.key;
        if (!hit.common) advancedOpen = true;
      }
    } finally {
      busy = false;
    }
  }

  function addUnknown() {
    unknown = { ...unknown, "": "" };
    advancedOpen = true;
  }

  function removeUnknown(key) {
    const next = { ...unknown };
    delete next[key];
    unknown = next;
  }

  function renameUnknown(previous, next) {
    if (previous === next) return;
    const entries = Object.entries(unknown);
    const rebuilt = {};
    for (const [key, value] of entries) {
      rebuilt[key === previous ? next : key] = value;
    }
    unknown = rebuilt;
  }

  function deviceOptions(field) {
    const devices = chat.devices.serial ?? [];
    const current = values[field.key];
    const options = devices.map((device) => ({
      value: device.device,
      label: device.description
        ? `${device.device} — ${device.description}`
        : device.device,
    }));
    if (current && !options.some((option) => option.value === current)) {
      options.unshift({ value: current, label: current });
    }
    return options;
  }

  onMount(() => {
    if (serialFields) chat.listDevices().catch(() => {});
  });
</script>

<div class="interface-form">
  {#snippet fieldWidget(field)}
    <div class="field-block" class:span-2={field.widget === "mono" || field.widget === "secret"}>
      {#if field.widget === "bool"}
        <label class="form-check">
          <input type="checkbox" bind:checked={values[field.key]} />
          {field.label}
        </label>
      {:else}
        <label class="field-label">
          {field.label}{#if field.units}<span class="field-units">({field.units})</span>{/if}
          {#if field.widget === "select"}
            <select class="field" bind:value={values[field.key]}>
              {#if values[field.key] === ""}<option value="">—</option>{/if}
              {#each field.choices ?? [] as choice (choice)}
                <option value={choice}>{choice}</option>
              {/each}
            </select>
          {:else if field.widget === "serial"}
            <select class="field" bind:value={values[field.key]}>
              <option value="">Choose a device…</option>
              {#each deviceOptions(field) as option (option.value)}
                <option value={option.value}>{option.label}</option>
              {/each}
            </select>
            {#if chat.devices.reason}
              <small class="field-help">{chat.devices.reason}. Paste the path instead.</small>
              <input class="field mono" bind:value={values[field.key]} placeholder="/dev/ttyUSB0" />
            {/if}
          {:else if field.widget === "secret"}
            <span class="secret-row">
              <input
                class="field mono"
                type={revealed[field.key] ? "text" : "password"}
                bind:value={values[field.key]}
                placeholder={field.placeholder ?? ""}
              />
              <button
                class="btn ghost"
                type="button"
                onclick={() => (revealed = { ...revealed, [field.key]: !revealed[field.key] })}
              >
                {revealed[field.key] ? "Hide" : "Show"}
              </button>
            </span>
          {:else if field.widget === "int" || field.widget === "port"}
            <input
              class="field"
              type="number"
              step={field.widget === "port" ? 1 : "any"}
              min={field.widget === "port" ? 1 : undefined}
              bind:value={values[field.key]}
              placeholder={field.placeholder ?? ""}
            />
          {:else}
            <input
              class="field"
              class:mono={field.widget === "mono"}
              type="text"
              bind:value={values[field.key]}
              placeholder={field.placeholder ?? ""}
              spellcheck="false"
            />
          {/if}
        </label>
      {/if}
      {#if field.help}<small class="field-help">{field.help}</small>{/if}
      {#if errorField === field.key}<small class="field-error">{error}</small>{/if}
    </div>
  {/snippet}

  <div class="net-section-head">
    <h2>{row ? `Edit ${row.name}` : `Add ${type?.label || "interface"}`}</h2>
    <button class="link-button" onclick={onCancel} disabled={busy}>Back</button>
  </div>
  {#if type?.summary}
    <p class="net-form-summary">{type.summary}</p>
  {/if}

  {#if error}
    <div class="apply-error" role="alert">
      <strong>Reticulum did not accept this interface.</strong>
      <p>{error}</p>
      <div class="apply-error-actions">
        <button class="btn primary" onclick={save} disabled={busy}>Retry</button>
        <button class="btn ghost" onclick={onCancel} disabled={busy}>Revert</button>
      </div>
    </div>
  {/if}

  <div class="form-grid">
    <label class="field-label span-2">
      Name
      <input
        class="field"
        bind:value={name}
        disabled={Boolean(row)}
        placeholder="e.g. Home relay"
      />
      <small class="field-help">
        Shown in Reticulum and in <code>rns/config</code>. Unique, up to 64 characters.
      </small>
    </label>

    {#each fields.filter((field) => field.common) as field (field.key)}
      {#if visible(field)}
        {@render fieldWidget(field)}
      {/if}
    {/each}
  </div>

  <details class="advanced" bind:open={advancedOpen}>
    <summary>Advanced</summary>
    <div class="form-grid">
      {#each fields.filter((field) => !field.common) as field (field.key)}
        {#if visible(field)}
          {@render fieldWidget(field)}
        {/if}
      {/each}

      {#each Object.entries(unknown) as [key, value], index (index)}
        <div class="field-block span-2 unknown-row">
          <label class="field-label">
            Key
            <input
              class="field mono"
              value={key}
              oninput={(event) => renameUnknown(key, event.currentTarget.value)}
              placeholder="unknown_key"
            />
          </label>
          <label class="field-label">
            Value
            <input class="field mono" bind:value={unknown[key]} />
          </label>
          <button class="link-button danger" onclick={() => removeUnknown(key)}>
            Remove
          </button>
        </div>
      {/each}

      <div class="field-block span-2">
        <button class="link-button" onclick={addUnknown}>Add another key</button>
        <small class="field-help">
          Keys rchat does not know are kept and written back unchanged.
        </small>
      </div>
    </div>
  </details>

  <div class="dialog-actions">
    <button class="btn ghost" onclick={onCancel} disabled={busy}>Cancel</button>
    <button class="btn primary" onclick={save} disabled={busy}>
      {busy ? "Applying…" : row ? "Apply changes" : "Add interface"}
    </button>
  </div>
</div>
