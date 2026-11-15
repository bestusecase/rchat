<script>
  import { hashHue, initials } from "./util.js";

  let { id = "", label = "?", size = 40, status = null } = $props();
</script>

<span
  class="avatar"
  style="--size:{size}px; --hue:{hashHue(id || label)}"
  aria-hidden="true"
>
  <span class="avatar-text">{initials(label)}</span>
  {#if status}
    <span class="avatar-status" data-state={status}></span>
  {/if}
</span>
