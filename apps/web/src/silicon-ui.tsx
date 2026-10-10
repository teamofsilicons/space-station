// Silicon UI's native surfaces adapted to Solid. Upstream CSS and tokens are kept alongside
// this file; native controls preserve the station's keyboard, form and event behavior.
import { splitProps, type JSX } from "solid-js";
import { Dynamic } from "solid-js/web";
import button from "./silicon-ui/button.module.css";
import input from "./silicon-ui/input.module.css";
import badge from "./silicon-ui/badge.module.css";
import card from "./silicon-ui/card.module.css";

const classes = (base: string, name?: string, flags?: Record<string, boolean | undefined>) =>
  [base, name, ...Object.entries(flags || {}).filter(([, enabled]) => enabled).map(([key]) => key)].filter(Boolean).join(" ");

export function SiliconButton(props: JSX.ButtonHTMLAttributes<HTMLButtonElement>) {
  const [local, rest] = splitProps(props, ["class", "classList"]);
  const variant = () => /(^|\s)primary(\s|$)/.test(local.class || "") ? button.primary
    : /(^|\s)(ghost|icon|link)(\s|$)/.test(local.class || "") ? button.ghost : button.secondary;
  return <button {...rest} class={classes(`${button.button} ${variant()} silicon-button`, local.class, local.classList)} />;
}
export function SiliconInput(props: JSX.InputHTMLAttributes<HTMLInputElement>) {
  const [local, rest] = splitProps(props, ["class", "classList"]);
  return <input {...rest} class={classes(["checkbox", "radio", "range", "color"].includes(props.type || "") ? "" : `${input.input} silicon-input`, local.class, local.classList)} />;
}
export function SiliconBadge(props: JSX.HTMLAttributes<HTMLSpanElement>) {
  const [local, rest] = splitProps(props, ["class", "classList"]);
  return <span {...rest} class={classes(badge.badge, local.class, local.classList)} />;
}
export function SiliconCard(props: JSX.HTMLAttributes<HTMLElement> & { as?: "div" | "article" | "section" }) {
  const [local, rest] = splitProps(props, ["class", "classList", "as"]);
  return <Dynamic component={local.as || "section"} {...rest} class={classes(card.card, local.class, local.classList)} />;
}
