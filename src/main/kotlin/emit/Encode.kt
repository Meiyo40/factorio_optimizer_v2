package emit

import java.util.zip.Deflater
import java.io.ByteArrayOutputStream
import java.util.Base64

fun compressAndEncode(jsonString: String): String {
    // 1. Get bytes from the string
    val inputBytes = jsonString.toByteArray(Charsets.UTF_8)

    // 2. Compress the bytes using zlib (DEFLATE)
    val deflater = Deflater()
    deflater.setInput(inputBytes)
    deflater.finish()

    val outputStream = ByteArrayOutputStream()
    val buffer = ByteArray(1024)
    while (!deflater.finished()) {
        val count = deflater.deflate(buffer)
        outputStream.write(buffer, 0, count)
    }
    outputStream.close()
    val compressedBytes = outputStream.toByteArray()

    // 3. Encode the *compressed* bytes to Base64
    val base64String = Base64.getEncoder().encodeToString(compressedBytes)

    // 4. Add the version '0'
    return "0" + base64String
}

/** Inverse of [compressAndEncode]: strips the version byte, base64-decodes and inflates to JSON. */
fun decodeToJson(blueprintString: String): String {
    val trimmed = blueprintString.trim()
    require(trimmed.startsWith("0")) { "Unknown blueprint string version '${trimmed.firstOrNull()}'" }
    val compressed = Base64.getDecoder().decode(trimmed.substring(1))
    val inflater = java.util.zip.Inflater()
    inflater.setInput(compressed)
    val out = ByteArrayOutputStream()
    val buffer = ByteArray(4096)
    while (!inflater.finished()) {
        val n = inflater.inflate(buffer)
        if (n == 0 && inflater.needsInput()) error("Truncated blueprint string")
        out.write(buffer, 0, n)
    }
    inflater.end()
    return out.toString(Charsets.UTF_8)
}
