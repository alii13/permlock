<template>
  <div>{{ title }}</div>
</template>

<script setup lang="ts">
import { connect } from "vue-dep";
const title: string = "hi";
connect();
</script>
