import { useEffect, useRef } from 'react';

export function useDownloads() {
  const resources = useRef(new Map<string, ReturnType<typeof setTimeout>>());
  useEffect(
    () => () => {
      for (const [url, timer] of resources.current) {
        clearTimeout(timer);
        URL.revokeObjectURL(url);
      }
      resources.current.clear();
    },
    [],
  );
  return (blob: Blob, name: string) => {
    const url = URL.createObjectURL(blob);
    const link = document.createElement('a');
    link.href = url;
    link.download = name;
    link.click();
    resources.current.set(
      url,
      setTimeout(() => {
        URL.revokeObjectURL(url);
        resources.current.delete(url);
      }, 1000),
    );
  };
}
