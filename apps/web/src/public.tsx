// The bar over the public pages: the brand, the docs and inspirations, and the way into the station.
import { Icon, Mark } from "./icons";

export function PublicBar() {
  return (
    <header className="public-bar">
      <a className="brand" href="/">
        <Mark size={20} />
        Space Station
      </a>
      <nav>
        <a href="/docs">Docs</a>
        <a href="/inspirations">Inspirations</a>
      </nav>
      <a className="button primary small" href="/">
        Open the station <Icon name="forward" />
      </a>
    </header>
  );
}
