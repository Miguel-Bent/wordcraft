package wordcraft.android;

import android.app.NativeActivity;
import android.content.ClipData;
import android.content.ClipDescription;
import android.content.ClipboardManager;
import android.content.ContentResolver;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.os.Bundle;
import android.os.CancellationSignal;
import android.os.ParcelFileDescriptor;
import android.print.PageRange;
import android.print.PrintAttributes;
import android.print.PrintDocumentAdapter;
import android.print.PrintDocumentInfo;
import android.print.PrintManager;
import android.provider.OpenableColumns;

import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.ArrayList;
import java.util.List;

/**
 * The activity WordCraft runs in. It is a NativeActivity (the interface is Rust), plus the one
 * thing the native side can't do: the system file picker (Storage Access Framework) reports its
 * result to an activity, not to native code.
 *
 * Documents travel as bytes, as in the browser build: an open reads the picked file and hands
 * its bytes to Rust; a save takes the bytes Rust staged and writes them to the chosen location.
 * Every native call here only queues an event for the UI thread, so none of them blocks it.
 */
public class WordCraftActivity extends NativeActivity {
    /** Request codes: open requests are OPEN_CODE + id, save requests SAVE_CODE + id (ids below 0x1000). */
    private static final int OPEN_CODE = 0x1000;
    private static final int SAVE_CODE = 0x2000;

    /** Outcomes passed to {@link #nativeOnSaved}. */
    private static final int STATUS_SAVED = 0;
    private static final int STATUS_CANCELLED = 1;
    private static final int STATUS_FAILED = 2;

    static {
        System.loadLibrary("wordcraft_tablet");
    }

    /** Called from Rust (UI thread): shows the system picker for one or more documents. */
    public void openDocument(final int requestId, final String[] mimeTypes, final boolean multiple) {
        runOnUiThread(() -> {
            Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
            intent.addCategory(Intent.CATEGORY_OPENABLE);
            intent.setType("*/*");
            if (mimeTypes.length > 0 && !mimeTypes[0].equals("*/*")) {
                intent.putExtra(Intent.EXTRA_MIME_TYPES, mimeTypes);
            }
            intent.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, multiple);
            startActivityForResult(intent, OPEN_CODE + requestId);
        });
    }

    /** Called from Rust (UI thread): shows the system "save as" picker with a suggested name. */
    public void createDocument(final int requestId, final String name, final String mimeType) {
        runOnUiThread(() -> {
            Intent intent = new Intent(Intent.ACTION_CREATE_DOCUMENT);
            intent.addCategory(Intent.CATEGORY_OPENABLE);
            intent.setType(mimeType);
            intent.putExtra(Intent.EXTRA_TITLE, name);
            startActivityForResult(intent, SAVE_CODE + requestId);
        });
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode >= OPEN_CODE && requestCode < SAVE_CODE) {
            onPicked(requestCode - OPEN_CODE, resultCode, data);
        } else if (requestCode >= SAVE_CODE) {
            onSaveLocationChosen(requestCode - SAVE_CODE, resultCode, data);
        }
    }

    private void onPicked(final int requestId, int resultCode, Intent data) {
        final List<Uri> uris = pickedUris(data);
        if (resultCode != RESULT_OK || uris.isEmpty()) {
            return;
        }
        new Thread(() -> {
            ContentResolver resolver = getContentResolver();
            for (Uri uri : uris) {
                try (java.io.InputStream input = resolver.openInputStream(uri)) {
                    if (input == null) {
                        continue;
                    }
                    nativeOnPicked(requestId, displayName(uri), readAll(input));
                } catch (IOException error) {
                    // A file that can't be read is skipped; the app shows nothing for it.
                }
            }
        }).start();
    }

    private void onSaveLocationChosen(final int requestId, int resultCode, Intent data) {
        final Uri uri = data == null ? null : data.getData();
        if (resultCode != RESULT_OK || uri == null) {
            nativeOnSaved(requestId, STATUS_CANCELLED, "");
            return;
        }
        new Thread(() -> {
            byte[] bytes = nativeTakeSaveBytes(requestId);
            if (bytes == null) {
                nativeOnSaved(requestId, STATUS_FAILED, "the document is no longer available to save");
                return;
            }
            try (OutputStream output = getContentResolver().openOutputStream(uri, "wt")) {
                if (output == null) {
                    nativeOnSaved(requestId, STATUS_FAILED, "the chosen file can't be opened for writing");
                    return;
                }
                output.write(bytes);
                nativeOnSaved(requestId, STATUS_SAVED, "");
            } catch (IOException error) {
                String message = error.getMessage();
                nativeOnSaved(requestId, STATUS_FAILED, message == null ? "the file could not be written" : message);
            }
        }).start();
    }

    private static List<Uri> pickedUris(Intent data) {
        List<Uri> uris = new ArrayList<>();
        if (data == null) {
            return uris;
        }
        ClipData clip = data.getClipData();
        if (clip != null) {
            for (int i = 0; i < clip.getItemCount(); i++) {
                Uri uri = clip.getItemAt(i).getUri();
                if (uri != null) {
                    uris.add(uri);
                }
            }
        } else if (data.getData() != null) {
            uris.add(data.getData());
        }
        return uris;
    }

    private String displayName(Uri uri) {
        try (Cursor cursor = getContentResolver().query(uri, null, null, null, null)) {
            if (cursor != null && cursor.moveToFirst()) {
                int column = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME);
                if (column >= 0) {
                    String name = cursor.getString(column);
                    if (name != null) {
                        return name;
                    }
                }
            }
        }
        String last = uri.getLastPathSegment();
        return last == null ? "document" : last;
    }

    private static byte[] readAll(java.io.InputStream input) throws IOException {
        java.io.ByteArrayOutputStream out = new java.io.ByteArrayOutputStream();
        byte[] buffer = new byte[64 * 1024];
        int read;
        while ((read = input.read(buffer)) != -1) {
            out.write(buffer, 0, read);
        }
        return out.toByteArray();
    }

    // ---- Clipboard (the system's ClipboardManager; called from Rust on the UI thread) ----

    private ClipboardManager clipboard() {
        return (ClipboardManager) getSystemService(CLIPBOARD_SERVICE);
    }

    /**
     * The first item of the primary clip as plain text, or null. A picture copied as a file has
     * only a URI: coerceToText would read that file as text, so a picture is not text here.
     */
    public String clipboardText() {
        ClipData clip = primaryClip();
        if (clip == null) {
            return null;
        }
        ClipData.Item item = clip.getItemAt(0);
        if (item.getText() == null && !hasTextType(clip.getDescription())) {
            return null;
        }
        CharSequence text = item.coerceToText(this);
        return text == null ? null : text.toString();
    }

    private static boolean hasTextType(ClipDescription description) {
        for (int i = 0; i < description.getMimeTypeCount(); i++) {
            if (description.getMimeType(i).startsWith("text/")) {
                return true;
            }
        }
        return false;
    }

    /** The first item's HTML (another app's formatted copy), or null. */
    public String clipboardHtml() {
        ClipData clip = primaryClip();
        return clip == null ? null : clip.getItemAt(0).getHtmlText();
    }

    /** The MIME types of the primary clip, one per line; empty when there is no clip. */
    public String clipboardMimeTypes() {
        ClipData clip = primaryClip();
        if (clip == null) {
            return "";
        }
        ClipDescription description = clip.getDescription();
        StringBuilder types = new StringBuilder();
        for (int i = 0; i < description.getMimeTypeCount(); i++) {
            types.append(description.getMimeType(i)).append('\n');
        }
        return types.toString();
    }

    /** Puts text (and its HTML, when given) on the clipboard. */
    public boolean setClipboard(String html, String text) {
        ClipboardManager manager = clipboard();
        if (manager == null) {
            return false;
        }
        ClipData data = (html == null || html.isEmpty())
                ? ClipData.newPlainText("WordCraft", text)
                : ClipData.newHtmlText("WordCraft", text, html);
        manager.setPrimaryClip(data);
        return true;
    }

    private ClipData primaryClip() {
        ClipboardManager manager = clipboard();
        if (manager == null || !manager.hasPrimaryClip()) {
            return null;
        }
        ClipData clip = manager.getPrimaryClip();
        return clip == null || clip.getItemCount() == 0 ? null : clip;
    }

    /** The picture on the clipboard (a copied image, or a picture file) as its encoded bytes, or null. */
    public byte[] clipboardImage() {
        ClipData clip = primaryClip();
        if (clip == null) {
            return null;
        }
        ClipDescription description = clip.getDescription();
        boolean picture = false;
        for (int i = 0; i < description.getMimeTypeCount(); i++) {
            picture |= description.getMimeType(i).startsWith("image/");
        }
        Uri uri = clip.getItemAt(0).getUri();
        if (!picture || uri == null) {
            return null;
        }
        try (InputStream input = getContentResolver().openInputStream(uri)) {
            return input == null ? null : readAll(input);
        } catch (IOException error) {
            return null;
        }
    }

    /** Opens the system print dialog for a PDF (the bytes WordCraft printed to). */
    public void printPdf(final byte[] pdf, final String jobName) {
        runOnUiThread(() -> {
            PrintManager manager = (PrintManager) getSystemService(PRINT_SERVICE);
            if (manager == null) {
                return;
            }
            manager.print(jobName, new PrintDocumentAdapter() {
                @Override
                public void onLayout(PrintAttributes oldAttributes, PrintAttributes newAttributes,
                                     CancellationSignal cancellationSignal, LayoutResultCallback callback, Bundle extras) {
                    if (cancellationSignal.isCanceled()) {
                        callback.onLayoutCancelled();
                        return;
                    }
                    PrintDocumentInfo info = new PrintDocumentInfo.Builder(jobName)
                            .setContentType(PrintDocumentInfo.CONTENT_TYPE_DOCUMENT)
                            .setPageCount(PrintDocumentInfo.PAGE_COUNT_UNKNOWN)
                            .build();
                    callback.onLayoutFinished(info, !newAttributes.equals(oldAttributes));
                }

                @Override
                public void onWrite(PageRange[] pages, ParcelFileDescriptor destination,
                                    CancellationSignal cancellationSignal, WriteResultCallback callback) {
                    try (FileOutputStream output = new FileOutputStream(destination.getFileDescriptor())) {
                        output.write(pdf);
                        output.flush();
                        callback.onWriteFinished(new PageRange[] {PageRange.ALL_PAGES});
                    } catch (IOException error) {
                        callback.onWriteFailed(error.getMessage());
                    }
                }
            }, null);
        });
    }

    /** Implemented in Rust (platform.rs): a picked file's name and bytes, queued for the app. */
    private static native void nativeOnPicked(int requestId, String name, byte[] bytes);

    /** Implemented in Rust: the bytes a save staged, or null when there are none. */
    private static native byte[] nativeTakeSaveBytes(int requestId);

    /** Implemented in Rust: how a save ended; {@code message} says why when it failed. */
    private static native void nativeOnSaved(int requestId, int status, String message);
}
