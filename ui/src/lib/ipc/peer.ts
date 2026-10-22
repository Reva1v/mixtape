// Stats sync with the phone (src-tauri/src/commands/peer.rs): the pairing QR code, the
// pairing state and the Windows Firewall rule. Changes arrive as `peer:status`.

import { invoke, on } from "./index";
import type { PairStart } from "./bindings/PairStart";
import type { PeerStatus } from "./bindings/PeerStatus";

export type { PairStart, PeerStatus };
export type { Firewall } from "./bindings/Firewall";

export const peerStatus = () => invoke<PeerStatus>("peer_status");
/** A new code, good for 5 minutes; the PC listens for the phone until it pairs or the code is cancelled. */
export const peerPairStart = () => invoke<PairStart>("peer_pair_start");
export const peerPairCancel = () => invoke<PeerStatus>("peer_pair_cancel");
export const peerUnpair = () => invoke<PeerStatus>("peer_unpair");
/** Adds the firewall rule through the Windows UAC prompt; rejects when the user says no. */
export const peerFirewallAdd = () => invoke<PeerStatus>("peer_firewall_add");
export const onPeerStatus = (handler: (s: PeerStatus) => void) => on<PeerStatus>("peer:status", handler);
