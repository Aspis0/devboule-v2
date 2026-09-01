import type { Helper } from "./helper";
import { helper } from "./helper";
import { helper as repeated } from "./helper";

export const main: Helper = helper ?? repeated;
