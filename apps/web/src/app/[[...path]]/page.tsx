// Every other address is the station: the browser decides what it shows (src/main.tsx).
import { loadDocs } from "../../docs-content";
import { App } from "../../main";

export default function Page() {
  return <App docs={loadDocs()} />;
}
