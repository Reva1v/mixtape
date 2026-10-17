import { base } from "./base";
import { screens } from "./screens";
import { ui } from "./ui";
import { updates } from "./updates";
import { iconStyle } from "./iconStyle";
import { tray } from "./tray";
import { stats } from "./stats";
import { integrations } from "./integrations";

export const en = { ...base, ...screens, ...ui, ...tray, ...stats, ...integrations, updates, iconStyle };
