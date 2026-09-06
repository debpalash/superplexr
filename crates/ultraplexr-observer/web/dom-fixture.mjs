// Small DOM surface shared by renderer tests; no browser or dependencies.
export function domFixture() {
  const document = {activeElement:null};
  class Element {
    constructor(tag) {
      this.tagName = tag; this.children = []; this.parentNode = null;
      this.dataset = {}; this.attributes = {}; this.hidden = false; this.disabled = false;
      this.content = "";
      this.style = {setProperty(name,value) { this[name] = value; }, removeProperty(name) {
        delete this[name]; delete this[name.replace(/-([a-z])/g, (_,letter) => letter.toUpperCase())];
      }};
      const classes = new Set();
      this.classList = {add:name => classes.add(name), remove:name => classes.delete(name), contains:name => classes.has(name),
        toggle:(name,force) => { const present = force ?? !classes.has(name); if (present) classes.add(name); else classes.delete(name); return present; }};
    }
    get parentElement() { return this.parentNode; }
    get firstElementChild() { return this.children[0] || null; }
    get lastElementChild() { return this.children.at(-1) || null; }
    get nextElementSibling() { return this.parentNode?.children[this.parentNode.children.indexOf(this)+1] || null; }
    get textContent() { return this.content + this.children.map(child => child.textContent).join(""); }
    set textContent(value) { this.replaceChildren(); this.content = String(value); }
    append(...nodes) { for (const node of nodes) this.insertBefore(node,null); }
    insertBefore(node,before) {
      if (node.tagName === "#fragment") { for (const child of [...node.children]) this.insertBefore(child,before); return; }
      if (node === before) return;
      node.remove(); node.parentNode = this;
      const index = before === null ? this.children.length : this.children.indexOf(before);
      if (index < 0) throw new Error("insertBefore target is not a child");
      this.children.splice(index,0,node);
    }
    replaceChildren(...nodes) { for (const child of [...this.children]) child.remove(); this.content = ""; this.append(...nodes); }
    remove() {
      if (this.parentNode) {
        const index = this.parentNode.children.indexOf(this); this.parentNode.children.splice(index,1); this.parentNode = null;
        if (this.contains(document.activeElement)) document.activeElement = null;
      }
    }
    contains(node) { return node === this || this.children.some(child => child.contains(node)); }
    querySelectorAll(tag) { return this.children.flatMap(child => [...(child.tagName === tag ? [child] : []), ...child.querySelectorAll(tag)]); }
    setAttribute(name,value) { this.attributes[name] = String(value); }
    getAttribute(name) { return this.attributes[name] ?? null; }
    focus() { document.activeElement = this; }
    set innerHTML(_) { throw new Error("Terminal and session data must never become HTML"); }
  }
  document.createElement = tag => new Element(tag);
  document.createDocumentFragment = () => new Element("#fragment");
  return {document};
}
