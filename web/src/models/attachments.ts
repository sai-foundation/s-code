function fileBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.addEventListener("error", () => reject(reader.error || new Error(`Could not read ${file.name}`)));
    reader.addEventListener("load", () => {
      const value = String(reader.result || "");
      const separator = value.indexOf(",");
      if (separator < 0) reject(new Error(`Could not encode ${file.name}`));
      else resolve(value.slice(separator + 1));
    });
    reader.readAsDataURL(file);
  });
}

// Reading a File is asynchronous. Revalidate its originating account and
// session before allowing the caller to submit bytes to the daemon.
export async function prepareAttachment(file: File, stillCurrent: () => boolean) {
  const assertCurrent = () => {
    if (!stillCurrent()) throw new DOMException("Account or session changed while reading an attachment", "AbortError");
  };
  assertCurrent();
  try {
    const content = await fileBase64(file);
    assertCurrent();
    return { file_name: file.name, media_type: file.type || "application/octet-stream", content_base64: content };
  } catch (error) {
    assertCurrent();
    throw error;
  }
}
