// Executed only inside the opaque-origin preview frame, after its resource policy.
(() => {
  try {
    const payload = JSON.parse(document.querySelector('#deep-project-data').textContent);
    const files = new Map(payload.files.map((file) => [file.path, file.content]));
    const origin = 'https://deep-project.invalid/';
    const resolve = (reference, from = payload.entry) => {
      const url = new URL(reference, new URL(from, origin));
      return url.origin === new URL(origin).origin
        ? decodeURIComponent(url.pathname.slice(1))
        : null;
    };
    const images = new Map();
    const decoded = new Map();
    for (const [path, data] of Object.entries(payload.resources ?? {})) {
      if (typeof data !== 'string') throw new Error('Invalid captured image payload.');
      let url = decoded.get(data);
      if (!url) {
        const header = /^data:(image\/(?:png|jpeg));base64,/.exec(data);
        if (!header) throw new Error('Unsupported captured image encoding.');
        const binary = atob(data.slice(header[0].length));
        const bytes = new Uint8Array(binary.length);
        for (let index = 0; index < binary.length; index++) bytes[index] = binary.charCodeAt(index);
        url = URL.createObjectURL(new Blob([bytes], { type: header[1] }));
        decoded.set(data, url);
      }
      images.set(path, url);
    }
    const imageUrl = (reference, from = payload.entry, required = false) => {
      const path = resolve(reference, from);
      if (path === null) return reference;
      if (!images.has(path) && path.toLowerCase().endsWith('.svg') && files.has(path)) {
        images.set(
          path,
          URL.createObjectURL(new Blob([files.get(path)], { type: 'image/svg+xml' })),
        );
      }
      if (images.has(path))
        return images.get(path) + new URL(reference, new URL(from, origin)).hash;
      if (required || /\.(?:png|jpe?g|svg)$/i.test(path))
        throw new Error(`Project image "${path}" is missing or is not bound to an asset.`);
      return reference;
    };
    const cssReference = (text) =>
      text
        .replace(/\\(?:\r\n|[\r\n\f])/g, '')
        .replace(/\\([0-9a-f]{1,6}[ \t\r\n\f]?|[\s\S])/gi, (_, escaped) => {
          if (!/^[0-9a-f]/i.test(escaped)) return escaped;
          const code = Number.parseInt(escaped.trim(), 16);
          return code > 0 && code <= 0x10ffff ? String.fromCodePoint(code) : '\ufffd';
        });
    const rewriteCss = (text, from = payload.entry) => {
      const escapeEnd = (start) => {
        let end = start + 1;
        if (/[0-9a-f]/i.test(text[end] ?? '')) {
          let digits = 0;
          while (digits < 6 && /[0-9a-f]/i.test(text[end] ?? '')) {
            end++;
            digits++;
          }
          if (text[end] === '\r' && text[end + 1] === '\n') return end + 2;
          return /\s/.test(text[end] ?? '') ? end + 1 : end;
        }
        return text[end] === '\r' && text[end + 1] === '\n'
          ? end + 2
          : Math.min(end + 1, text.length);
      };
      const stringEnd = (start) => {
        const quote = text[start++];
        while (start < text.length) {
          if (text[start] === '\\') start = escapeEnd(start);
          else if (text[start++] === quote) return start;
        }
        return text.length;
      };
      const spaceEnd = (start) => {
        while (start < text.length) {
          if (/\s/.test(text[start])) start++;
          else if (text.startsWith('/*', start)) {
            const end = text.indexOf('*/', start + 2);
            start = end < 0 ? text.length : end + 2;
          } else break;
        }
        return start;
      };
      let output = '';
      let copied = 0;
      let offset = 0;
      while (offset < text.length) {
        if (text.startsWith('/*', offset)) {
          const end = text.indexOf('*/', offset + 2);
          offset = end < 0 ? text.length : end + 2;
          continue;
        }
        if (text[offset] === '"' || text[offset] === "'") {
          offset = stringEnd(offset);
          continue;
        }
        if (
          text.slice(offset, offset + 4).toLowerCase() !== 'url(' ||
          /[\\\w\u0080-\uFFFF-]/.test(text[offset - 1] ?? '')
        ) {
          offset++;
          continue;
        }
        let end = spaceEnd(offset + 4);
        let raw;
        if (text[end] === '"' || text[end] === "'") {
          const start = end;
          end = stringEnd(start);
          if (text[end - 1] !== text[start]) {
            offset = end;
            continue;
          }
          raw = text.slice(start + 1, end - 1);
          end = spaceEnd(end);
        } else {
          const start = end;
          let valid = true;
          while (end < text.length && text[end] !== ')') {
            if (text[end] === '\\') end = escapeEnd(end);
            else if (/\s/.test(text[end])) {
              end = spaceEnd(end);
              break;
            } else if (text[end] === '(' || text[end] === '"' || text[end] === "'") {
              valid = false;
              break;
            } else end++;
          }
          raw = text.slice(start, end).trim();
          if (!valid) {
            offset = Math.max(end, offset + 4);
            continue;
          }
        }
        if (text[end] !== ')') {
          offset = Math.max(end, offset + 4);
          continue;
        }
        const reference = cssReference(raw);
        const url = imageUrl(reference, from);
        if (url !== reference) {
          output += text.slice(copied, offset) + `url(${JSON.stringify(url)})`;
          copied = end + 1;
        }
        offset = end + 1;
      }
      return output + text.slice(copied);
    };
    const rewriteSrcset = (text) => {
      const candidates = [];
      let offset = 0;
      while (offset < text.length) {
        while (/[\s,]/.test(text[offset] ?? '') && offset < text.length) offset++;
        if (offset >= text.length) break;
        const start = offset;
        while (offset < text.length && !/\s/.test(text[offset])) offset++;
        let url = text.slice(start, offset);
        let descriptor = '';
        if (url.endsWith(',')) {
          url = url.replace(/,+$/, '');
        } else {
          const start = offset;
          while (offset < text.length && text[offset] !== ',') offset++;
          descriptor = text.slice(start, offset).trim();
          if (descriptor && !/^(?:\d+(?:\.\d+)?x|\d+w)$/.test(descriptor))
            throw new Error(
              'Preview supports literal image srcset candidates with width or density descriptors.',
            );
        }
        candidates.push(
          `${imageUrl(url, payload.entry, true)}${descriptor ? ` ${descriptor}` : ''}`,
        );
      }
      return candidates.join(', ');
    };
    const doc = new DOMParser().parseFromString(
      payload.policy + files.get(payload.entry),
      'text/html',
    );
    for (const baseElement of doc.querySelectorAll('base')) baseElement.remove();
    for (const script of doc.querySelectorAll('script')) {
      if (script.type === 'module' || script.type === 'importmap') {
        throw new Error(
          'Preview supports classic scripts. Browser module builds are not supported yet.',
        );
      }
      if (!script.hasAttribute('src')) continue;
      const path = resolve(script.getAttribute('src'));
      if (path === null) continue; // The resource policy blocks external references.
      if (!files.has(path)) throw new Error('A linked project script is missing.');
      script.removeAttribute('src');
      script.removeAttribute('integrity');
      script.removeAttribute('crossorigin');
      script.removeAttribute('async');
      script.removeAttribute('defer');
      script.textContent = files.get(path).replace(/<\/script/gi, '<\\/script');
    }
    for (const link of doc.querySelectorAll('link')) {
      if (!link.rel.toLowerCase().split(/\s+/).includes('stylesheet')) continue;
      const path = resolve(link.getAttribute('href'));
      if (path === null) continue;
      if (!files.has(path)) throw new Error('A linked project stylesheet is missing.');
      const style = doc.createElement('style');
      style.textContent = rewriteCss(files.get(path), path).replace(/<\/style/gi, '<\\/style');
      link.replaceWith(style);
    }
    for (const img of doc.querySelectorAll('img, input[type="image"]')) {
      const reference = img.getAttribute('src');
      if (reference?.trim()) img.setAttribute('src', imageUrl(reference, payload.entry, true));
    }
    for (const source of doc.querySelectorAll('img[srcset], source[srcset]')) {
      source.setAttribute('srcset', rewriteSrcset(source.getAttribute('srcset')));
    }
    for (const video of doc.querySelectorAll('video[poster]')) {
      video.setAttribute('poster', imageUrl(video.getAttribute('poster'), payload.entry, true));
    }
    for (const image of doc.querySelectorAll('svg image')) {
      for (const attribute of ['href', 'xlink:href']) {
        const reference = image.getAttribute(attribute);
        if (reference?.trim())
          image.setAttribute(attribute, imageUrl(reference, payload.entry, true));
      }
    }
    for (const link of doc.querySelectorAll('link[rel]')) {
      if (link.rel.toLowerCase().split(/\s+/).includes('icon') && link.hasAttribute('href'))
        link.setAttribute('href', imageUrl(link.getAttribute('href'), payload.entry, true));
    }
    for (const style of doc.querySelectorAll('style'))
      style.textContent = rewriteCss(style.textContent);
    for (const node of doc.querySelectorAll('[style]'))
      node.setAttribute('style', rewriteCss(node.getAttribute('style')));
    const html = payload.policy + doc.documentElement.outerHTML;
    document.open();
    document.write(html);
    document.close();
  } catch (error) {
    const message = document.createElement('pre');
    message.textContent =
      error instanceof Error ? error.message : 'Cannot prepare this project preview.';
    document.body.replaceChildren(message);
  }
})();
