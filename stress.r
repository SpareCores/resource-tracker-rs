
# R Script to Temporarily Stress Machine (CPU)

duration_seconds <- 10 # Set stress duration
start_time       <- Sys.time()

cat("Starting stress test for", duration_seconds, "seconds...\n")

# Run loop until duration is reached
while (as.numeric(difftime(Sys.time(), start_time, units = "secs")) < duration_seconds) {
  # Perform heavy computation (Matrix Multiplication)
  mat1   <- matrix(runif(1e6), nrow = 1000)
  mat2   <- matrix(runif(1e6), nrow = 1000)
  result <- mat1 %*% mat2
  
  # Brief pause to keep script responsive, remove if maximum load is needed
  Sys.sleep(0.01) 
}

cat("Stress test complete.\n")
