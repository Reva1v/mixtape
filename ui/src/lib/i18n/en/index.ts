import { base } from "./base";
import { screens } from "./screens";
import { ui } from "./ui";
import { updates } from "./updates";
import { iconStyle } from "./iconStyle";
import { tray } from "./tray";
import { stats } from "./stats";

export const en = { ...base, ...screens, ...ui, ...tray, ...stats, updates, iconStyle };
