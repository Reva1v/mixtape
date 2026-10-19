import { base } from "./base";
import { screens } from "./screens";
import { ui } from "./ui";
import { updates } from "./updates";
import { iconStyle } from "./iconStyle";
import { tray } from "./tray";
import { stats } from "./stats";
import { integrations } from "./integrations";
import { normalize } from "./normalize";
import { smart } from "./smart";
import { spotify } from "./spotify";

export const uk = {
  ...base,
  ...screens,
  ...ui,
  ...tray,
  ...stats,
  ...integrations,
  ...normalize,
  ...smart,
  ...spotify,
  updates,
  iconStyle,
};
