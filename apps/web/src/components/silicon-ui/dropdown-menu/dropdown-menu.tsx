"use client";

import { Fragment, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { CSSProperties, ReactNode } from "react";
import * as DropdownPrimitive from "@radix-ui/react-dropdown-menu";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { ChevronDown } from "lucide-react";
import { MenuHighlight, useMenuHighlight } from "../lib/menu-highlight";
import { motionTokens } from "../lib/motion-tokens";
import styles from "./dropdown-menu.module.css";

export interface DropdownItem { label: string; onSelect?: () => void; disabled?: boolean; icon?: ReactNode; destructive?: boolean; separatorBefore?: boolean; }
export interface DropdownMenuProps { label: string; items: DropdownItem[]; icon?: ReactNode; }

/** A new trigger label rises in while the old one leaves, and the trigger width springs to the measured text instead of snapping. */
function TriggerLabel({ text }: { text: string }) {
  const reduced = useReducedMotion();
  const measure = useRef<HTMLSpanElement>(null);
  const measured = useRef<string | null>(null);
  const [size, setSize] = useState<{ width: number | "auto"; animate: boolean }>({ width: "auto", animate: false });
  useLayoutEffect(() => {
    const node = measure.current;
    if (!node) return;
    const observer = new ResizeObserver(([entry]) => {
      if (!entry) return;
      const current = node.textContent;
      // Only a text change morphs; the first measure and font swaps settle instantly.
      const animate = measured.current !== null && measured.current !== current;
      measured.current = current;
      setSize({ width: Math.ceil(entry.borderBoxSize?.[0]?.inlineSize ?? node.offsetWidth), animate });
    });
    observer.observe(node);
    return () => observer.disconnect();
  }, []);
  return <motion.span className={styles.label} initial={false} animate={{ width: size.width }} transition={size.animate && !reduced ? motionTokens.spring.morph : { duration: 0 }}>
    <span ref={measure} className={styles.labelMeasure} aria-hidden="true">{text}</span>
    <AnimatePresence mode="popLayout" initial={false}>
      <motion.span key={text} className={styles.labelText} initial={reduced ? false : { opacity: 0, y: "0.3em", filter: `blur(${motionTokens.blur.soft}px)` }} animate={{ opacity: 1, y: 0, filter: "blur(0px)" }} exit={reduced ? { opacity: 0, transition: { duration: 0 } } : { opacity: 0, y: "-0.3em", filter: `blur(${motionTokens.blur.subtle}px)`, transition: { duration: motionTokens.duration.fast, ease: [...motionTokens.ease.standard] } }} transition={{ duration: motionTokens.duration.standard, ease: [...motionTokens.ease.enter] }}>{text}</motion.span>
    </AnimatePresence>
  </motion.span>;
}

/** The menu's surface and items, shared by the trigger menu and the anchored one. */
function MenuContent({ items, menu: { highlight, contentProps }, align = "end", sideOffset = 6 }: { items: DropdownItem[]; menu: ReturnType<typeof useMenuHighlight>; align?: "start" | "end"; sideOffset?: number }) {
  return <DropdownPrimitive.Portal><DropdownPrimitive.Content className={styles.menu} sideOffset={sideOffset} align={align} collisionPadding={12} loop {...contentProps}>
    {/* One highlight glides between items for the pointer and jumps instantly for the keyboard. */}
    <MenuHighlight state={highlight} className={styles.highlight} />
    {items.map((item, index) => <Fragment key={item.label}>{item.separatorBefore && <DropdownPrimitive.Separator className={styles.separator}/>}<DropdownPrimitive.Item className={[styles.item, item.destructive ? styles.destructive : ""].filter(Boolean).join(" ")} data-tone={item.destructive ? "danger" : undefined} style={{ "--i": index } as CSSProperties} disabled={item.disabled} onSelect={item.onSelect}>{item.icon && <span className={styles.icon} aria-hidden="true">{item.icon}</span>}{item.label}</DropdownPrimitive.Item></Fragment>)}
  </DropdownPrimitive.Content></DropdownPrimitive.Portal>;
}

export function DropdownMenu({ label, items, icon }: DropdownMenuProps) {
  const menu = useMenuHighlight();
  return <DropdownPrimitive.Root onOpenChange={open => { if (open) menu.reset(); }}>
    <DropdownPrimitive.Trigger className={styles.trigger} type="button">{icon && <span className={styles.triggerIcon} aria-hidden="true">{icon}</span>}<TriggerLabel text={label}/><ChevronDown className={styles.chevron} size={15} strokeWidth={1.8} aria-hidden="true"/></DropdownPrimitive.Trigger>
    <MenuContent items={items} menu={menu} />
  </DropdownPrimitive.Root>;
}

export interface DropdownMenuAtProps { items: DropdownItem[]; /** Where the menu opens, in viewport pixels; null closes it. */ at: { x: number; y: number } | null; onClose: () => void; }

/** The same menu opened by the app at a point, such as a right click or the corner of the button that asked for it. */
export function DropdownMenuAt({ items, at, onClose }: DropdownMenuAtProps) {
  const menu = useMenuHighlight();
  // Each opening starts without a highlight, like the trigger menu's.
  useEffect(() => { if (at) menu.reset(); }, [at]); // eslint-disable-line react-hooks/exhaustive-deps
  return <DropdownPrimitive.Root open={!!at} onOpenChange={open => { if (!open) onClose(); }} modal={false}>
    <DropdownPrimitive.Trigger asChild><span aria-hidden="true" style={{ position: "fixed", left: at?.x ?? 0, top: at?.y ?? 0, width: 0, height: 0, pointerEvents: "none" }}/></DropdownPrimitive.Trigger>
    <MenuContent items={items} menu={menu} align="start" sideOffset={2} />
  </DropdownPrimitive.Root>;
}

export default DropdownMenu;
