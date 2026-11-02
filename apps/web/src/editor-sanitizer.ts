import createDOMPurify from 'dompurify';

// A private instance keeps Monaco's temporary hooks separate from other UI.
const sanitizer = createDOMPurify(window);
const blockResources = (node: Node) => {
  if (node instanceof Element) {
    for (const attribute of ['src', 'srcset', 'poster', 'href', 'xlink:href', 'style'])
      node.removeAttribute(attribute);
  }
};
const clearTemporaryHooks = sanitizer.removeAllHooks.bind(sanitizer);
// Monaco clears its hooks after every render. Reinstall the application policy
// so later hovers and completion details retain the same resource restriction.
sanitizer.removeAllHooks = () => {
  clearTemporaryHooks();
  sanitizer.addHook('afterSanitizeAttributes', blockResources);
};
sanitizer.removeAllHooks();

export default sanitizer;
