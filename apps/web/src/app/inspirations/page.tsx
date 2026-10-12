// The public inspirations page: the ideas shaping the station.
import type { Metadata } from "next";
import { Card } from "@/components/silicon-ui/card/card";
import { PublicBar } from "../../public";

export const metadata: Metadata = { title: "Inspirations · Space Station" };

const CARDS = [
  ["See the whole orbit", "Heart Aerospace", "SPACE / CLARITY", "A single calm frame can carry a complex system. We borrow the confidence: one strong surface, one next move."],
  ["Explain by unfolding", "Postevand", "FLOW / CONTEXT", "Product stories feel clearer when each chapter earns its place. Space Station turns ingest, shape, and notify into a visible route."],
  ["Make progress legible", "X Business", "SEQUENCE / MOMENTUM", "A numbered path reduces hesitation. Every table, window, and notification should feel like the next instrument on a flight deck."],
  ["Let the grid breathe", "Cosmos", "COLLECT / CONTRAST", "Small marks become a field. Dither, orbit lines, and quiet hover states give the interface a sense of depth without adding noise."],
  ["Use the edge as a cue", "Arc", "SURFACE / FOCUS", "The sidebar belongs to the room; the paper belongs to the work. A little separation makes the central canvas feel held."],
  ["Tell the truth plainly", "Making Software", "WORDS / WEIGHT", "The best line is often the one that says exactly what happened: records arrived, a view changed, a message went out."],
] as const;

export default function Inspirations() {
  return (
    <div className="standalone">
      <PublicBar />
      <main className="standalone-body inspirations">
        <span className="kicker">Field notes / 06</span>
        <h1>Inspirations for the station</h1>
        <p className="lede">A small atlas of the ideas shaping Space Station: systems that feel capable, calm, and a little unexpected.</p>
        <div className="card-grid">
          {CARDS.map(([title, source, kicker, text]) => (
            <Card key={title} title={title} meta={<span className="kicker accent">{kicker}</span>} description={text} status={source} />
          ))}
        </div>
      </main>
    </div>
  );
}
