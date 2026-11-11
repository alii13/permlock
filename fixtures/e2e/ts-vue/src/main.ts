import type { Shape } from "typedep";
import App from "./App.vue";
import { api } from "@/lib/api";

let socket: WebSocket | undefined;
const shape: Shape = { app: App, api, socket };
export const load = () => import("lazy");
export default shape;
