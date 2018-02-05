package fluence.crypto

import java.io.File
import java.nio.file.Files
import java.security
import java.security.KeyFactory
import java.security.spec.PKCS8EncodedKeySpec

import cats.Applicative
import cats.syntax.applicative._
import fluence.crypto.algorithm.{ Ecdsa, JavaAlgorithm }
import fluence.crypto.keypair.KeyPair
import org.bouncycastle.jce.spec.{ ECPrivateKeySpec, ECPublicKeySpec }

class FileKeyStorage[F[_] : Applicative](file: File) extends JavaAlgorithm {
  def readKeyPair: F[KeyPair] = {
    {
      val keyBytes = Files.readAllBytes(file.toPath)
      val keySpec = new PKCS8EncodedKeySpec(keyBytes)

      val keyFactory = KeyFactory.getInstance(Ecdsa.ECDSA)
      val privateKey = keyFactory.generatePrivate(keySpec)

      //todo avoid classOf
      val privSpec = keyFactory.getKeySpec(privateKey, classOf[ECPrivateKeySpec])
      val params = privSpec.getParams

      val q = params.getG.multiply(privSpec.getD)

      val pubSpec = new ECPublicKeySpec(q, params)
      val pubKey = keyFactory.generatePublic(pubSpec)

      JavaAlgorithm.jKeyPairToKeyPair(new security.KeyPair(pubKey, privateKey))
    }.pure[F]
  }

  def storeSecretKey(key: KeyPair.Secret): F[Unit] = {
    import java.io.FileOutputStream
    {
      if (!file.exists()) file.createNewFile() else throw new RuntimeException(file.getAbsolutePath + " already exists")
      val fos = new FileOutputStream(file)

      fos.write(key.value.toArray)
      fos.close()
    }.pure[F]
  }
}
