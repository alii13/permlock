import { get } from "fetcher";
export const api = (path: string): Promise<unknown> => get(path);
