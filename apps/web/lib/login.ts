export type IdentityKind = "carbon" | "silicon";

export function loginUrl(kind: IdentityKind, next = "/", org?: string, attempt?: string) {
  const query = new URLSearchParams({ identity_kind: kind, next });
  if (org) query.set("org", org);
  if (attempt) {
    query.set("attempt_id", attempt);
    query.set("display", "popup");
  }
  return `/api/auth/login?${query}`;
}

let cancelPending: (() => void) | undefined;
export function cancelLogin() { cancelPending?.(); }

/** Only the server callback can complete this attempt; a closed popup never signs us in. */
export function beginLogin(kind: IdentityKind, next = "/", org?: string): Promise<void> {
  const origin = window.location.origin;
  const destination = new URL(next, origin);
  if (!next.startsWith("/") || next.startsWith("//") || next.includes("\\") || destination.origin !== origin)
    return Promise.reject(new Error("The return destination must be a page in this application."));
  cancelLogin();
  const attempt = crypto.randomUUID();
  const popup = window.open(loginUrl(kind, next, org, attempt), "_blank", "popup=yes,width=560,height=720");
  if (!popup) {
    window.location.assign(loginUrl(kind, next, org));
    return Promise.resolve();
  }
  return new Promise((resolve, reject) => {
    let complete = false;
    const finish = (error?: string) => {
      if (complete) return;
      complete = true;
      window.removeEventListener("message", receive);
      clearInterval(closed);
      clearTimeout(timeout);
      cancelPending = undefined;
      popup.close();
      if (error) reject(new Error(error));
      else {
        // A fresh page verifies /me and discards the previous context's caches and requests.
        if (destination.pathname === window.location.pathname && destination.search === window.location.search) {
          window.location.hash = destination.hash;
          window.location.reload();
        } else window.location.assign(next);
        resolve();
      }
    };
    const receive = (event: MessageEvent) => {
      if (event.origin !== origin || event.source !== popup) return;
      const message = event.data;
      if (!message || typeof message !== "object" || Array.isArray(message) ||
          Object.keys(message).length !== 3 ||
          message.type !== "spacestation:login" || message.attempt_id !== attempt ||
          (message.status !== "success" && message.status !== "error")) return;
      finish(message.status === "error" ? "Sign-in was not completed. Please try again." : undefined);
    };
    const closed = setInterval(() => {
      if (popup.closed) finish("Sign-in cancelled. You can try again when ready.");
    }, 300);
    const timeout = setTimeout(() => finish("Sign-in timed out. Please try again."), 10 * 60 * 1000);
    cancelPending = () => finish("Sign-in cancelled. You can try again when ready.");
    window.addEventListener("message", receive);
  });
}
