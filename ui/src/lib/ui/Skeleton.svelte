<script lang="ts">
  let { class: cls = "" }: { class?: string } = $props();
</script>

<div class="animate-pulse rounded-md bg-fill-2 {cls}"></div>
