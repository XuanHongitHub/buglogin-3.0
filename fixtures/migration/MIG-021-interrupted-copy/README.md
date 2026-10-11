# MIG-021 Interrupted Copy

The staging directory simulates a process stop after the journal and some payload files exist. Resume restores only missing files. Existing tampered files and foreign entries fail closed; verified discard removes only the owned staging root.
